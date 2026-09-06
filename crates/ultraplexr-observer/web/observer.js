"use strict";
import {AccessEnded, followFeed} from "/stream.mjs";
import {followSessions, sessionLabel, sessionDetails} from "/sessions.mjs";
import {TerminalDisplay} from "/terminal.mjs";
import {FrameContinuityError, FrameDecoder} from "/frames.mjs";
import {SideViews} from "/side-views.mjs";
import {BrowserControl, bindTerminalInput} from "/control.mjs";
import {BrowserSearch} from "/search.mjs";
import {WorkflowInspector} from "/workflow.mjs";
const $ = id => document.getElementById(id);
const terminalDisplay = new TerminalDisplay($("output"));
let key = new URLSearchParams(location.hash.slice(1)).get("access") || "";
history.replaceState(null, "", location.pathname);
let selected = null, stream = null, paused = false, generation = 0;
let sessionStream = null;
let catalogReady = false;
let latestFrame = null, historyOffset = 0, historyGeneration = 0;
let allowControl = false;
const headers = () => ({ Authorization: "Bearer " + key });
const workflow = new WorkflowInspector($("workflow"), {headers});
const sideViews = new SideViews($("side-views"), {headers, onChange:updateSideViewButton,
  openMain:id => { if (catalogReady) selectSession(id); }});
function updateSideViewButton() {
  $("side-view").disabled = !selected || !catalogReady || !sideViews.available;
  $("side-view").title = sideViews.available ? "Observe this same session in an independent side view" : "Close a side view before opening another (maximum three)";
}
const search = new BrowserSearch({headers, onChange:updateSearch,
  onInspect:() => { historyGeneration++; paused = true; relinquish(); $("live").hidden = false; $("pause").textContent = "Resume updates"; },
  onFrame:frame => { renderFrame(frame); $("feed-status").textContent = "Read-only search history · host continues"; $("output").focus(); },
  onEnded:error => { stream?.abort(); clearFeed(); $("feed-status").textContent = error.message; }
});
let renderedMatches = 0, renderedFirst = null;
function updateSearch(state) {
  $("search-query").disabled = !state.session;
  $("search-start").disabled = !state.session;
  $("search-cancel").disabled = !state.running;
  $("search-status").textContent = state.status;
  const results = $("search-results");
  if (state.matches.length < renderedMatches || (state.matches[0] || null) !== renderedFirst) {
    results.replaceChildren(); renderedMatches = 0; renderedFirst = state.matches[0] || null;
  }
  for (const found of state.matches.slice(renderedMatches)) {
    const item = document.createElement("li"), button = document.createElement("button"), position = document.createElement("small");
    position.textContent = `Row ${found.line + 1}, column ${found.column + 1}`;
    button.append(position, document.createTextNode(found.preview));
    button.onclick = () => search.reveal(found);
    item.append(button); results.append(item);
  }
  renderedMatches = state.matches.length;
}
const control = new BrowserControl({headers, onChange:updateControl, onFailure:message => {
  search.cancel(true, "Connection changed; reattach before searching again");
  stream?.abort();
  $("feed-status").textContent = message;
  $("reattach").hidden = !selected;
}});
function updateControl() {
  if (!control.lease) { $("paste-preview").value = ""; $("paste-confirmation").hidden = true; }
  const live = !!latestFrame && latestFrame.status === "running" && !paused;
  $("output").classList.toggle("terminal-cursor-hidden", !live);
  $("interactive").hidden = !allowControl;
  $("claim").disabled = !control.surface || !!control.lease || !live || control.pending;
  $("release").disabled = !control.lease;
  $("terminal-input").disabled = !control.lease || !live;
  $("resize").disabled = !control.lease || !live;
  $("control-status").textContent = control.lease && live ? "You control this terminal" : "Observing · input disabled";
  $("interactive").classList.toggle("controlling", !!control.lease && live);
}
function relinquish() {
  if (control.lease) control.send({type:"release"});
  updateControl();
}
function updateBrowserMode(enabled) {
  allowControl = enabled;
  $("scope").textContent = allowControl ? "Scoped Controller Share · explicit takeover only" : "Read-only · input disabled";
  $("mode").textContent = allowControl ? "Browser" : "Observer";
  updateControl();
}
function renderSessions(terminals) {
  catalogReady = true;
  sideViews.reconcile(terminals);
  const nav = $("sessions");
  // Patch keyed nodes: title/status changes must not destroy keyboard focus.
  const buttons = new Map([...nav.querySelectorAll("button")].map(button => [button.dataset.id, button]));
  if (!buttons.size) nav.replaceChildren();
  let position = nav.firstElementChild;
  for (const terminal of terminals) {
    let button = buttons.get(terminal.session_id);
    if (!button) {
      button = document.createElement("button");
      button.dataset.id = terminal.session_id;
      button.append(document.createElement("span"), document.createElement("small"));
      button.onclick = () => selectSession(terminal.session_id);
    }
    const label = sessionLabel(terminal), details = sessionDetails(terminal);
    if (button.firstElementChild.textContent !== label) button.firstElementChild.textContent = label;
    if (button.lastElementChild.textContent !== details) button.lastElementChild.textContent = details;
    button.title = [label, terminal.directory, terminal.session_id].filter(Boolean).join("\n");
    if (button !== position) nav.insertBefore(button, position);
    position = button.nextElementSibling;
    buttons.delete(terminal.session_id);
  }
  for (const button of buttons.values()) button.remove();
  if (selected && !terminals.some(t => t.session_id === selected)) {
    stream?.abort(); selected = null; clearFeed();
    $("title").textContent = "Session no longer available"; $("copy").disabled = true; $("pause").disabled = true;
  }
  if (!terminals.length) nav.textContent = "No active sessions in this Share.";
  for (const button of nav.querySelectorAll("button")) {
    button.disabled = false;
    button.setAttribute("aria-current", String(button.dataset.id === selected));
  }
  $("connection").textContent = "Connected · " + terminals.length + " visible session" + (terminals.length === 1 ? "" : "s");
  updateSideViewButton();
}
function renderFrame(frame) {
  $("title").textContent = frame.title || selected?.slice(0, 8) || "Terminal";
  $("directory").textContent = frame.directory || "";
  const styled = terminalDisplay.render(frame, {cursor:!paused && frame.status === "running"});
  $("copy").disabled = false; $("history").disabled = false;
  $("feed-status").textContent = frame.status + " · frame " + frame.sequence + " · " + new Date().toLocaleTimeString() + (styled ? "" : " · text-only view");
  updateControl();
}
function clearFeed() {
  search.attach(null);
  control.detach(); $("reattach").hidden = true;
  historyGeneration++; $("live").hidden = true;
  terminalDisplay.clear(); latestFrame = null;
  $("directory").textContent = "";
  $("copy").disabled = true; $("pause").disabled = true; $("history").disabled = true;
}
async function selectSession(id) {
  stream?.abort();
  clearFeed();
  const controller = new AbortController(); stream = controller; selected = id;
  updateSideViewButton();
  const decoder = new FrameDecoder();
  const feedHeaders = {...headers(), "X-Ultraplexr-Frames":"row-delta-v1"};
  search.attach(id);
  paused = false; historyOffset = 0; latestFrame = null;
  $("pause").textContent = "Pause updates"; $("pause").disabled = false; $("live").hidden = true;
  terminalDisplay.clear(); $("title").textContent = "Connecting…";
  for (const button of $("sessions").querySelectorAll("button")) button.setAttribute("aria-current", String(button.dataset.id === id));
  try {
    await followFeed({
      url: "/sessions/" + encodeURIComponent(id) + "/events",
      headers: feedHeaders, signal: controller.signal,
      onRetry: (error, ms) => {
        decoder.reset(); latestFrame = null;
        search.cancel(true, "Connection changed; submit a new search after reconnect"); control.detach();
        if (error instanceof FrameContinuityError) {
          if (!feedHeaders["X-Ultraplexr-Frames"]) throw new AccessEnded("Invalid terminal frames; reconnect explicitly.");
          delete feedHeaders["X-Ultraplexr-Frames"];
        }
        $("feed-status").textContent = "Disconnected · reattaching to this session in " + ms / 1000 + "s. Input will stay disabled.";
      },
      onEvent: ({event, data}) => {
        if (controller.signal.aborted) return;
        if (event === "surface") {
          control.attach(data);
        } else if (event === "frame" || event === "frame-delta") {
          latestFrame = decoder.accept(event, data);
          if (!paused) renderFrame(latestFrame);
        } else if (event === "complete") {
          control.detach();
          $("feed-status").textContent = "Session ended · retained history is available";
        } else if (event === "reconnecting") {
          decoder.reset(); latestFrame = null;
          search.cancel(true, "Connection changed; submit a new search after reconnect");
          control.detach();
          $("feed-status").textContent = data;
        }
      }
    });
  } catch (error) {
    if (controller.signal.aborted) return;
    $("feed-status").textContent = error.message; clearFeed();
  }
}
async function connect() {
  workflow.setEnabled(false);
  const current = ++generation;
  catalogReady = false; sideViews.clear();
  sessionStream?.abort();
  stream?.abort(); clearFeed(); selected = null;
  const controller = new AbortController(); sessionStream = controller;
  $("sessions").replaceChildren();
  $("connect").hidden = true;
  $("connection").textContent = "Connecting to session updates…";
  try {
    await followSessions({headers:headers(), signal:controller.signal,
      onSessions:terminals => { if (current === generation) renderSessions(terminals); },
      onMode:enabled => { if (current === generation) updateBrowserMode(enabled); },
      onFeatures:features => { if (current === generation) workflow.setEnabled(features.workflow_read); },
      onStatus:message => {
        if (current !== generation) return;
        catalogReady = false; updateSideViewButton();
        $("connection").textContent = message;
        for (const button of $("sessions").querySelectorAll("button")) button.disabled = true;
      },
    });
  } catch (error) {
    if (current !== generation || controller.signal.aborted) return;
    workflow.setEnabled(false);
    catalogReady = false; sideViews.clear();
    stream?.abort(); selected = null; $("sessions").replaceChildren();
    clearFeed();
    $("connection").textContent = error.message; $("connect").hidden = false;
  }
}
$("connect").onsubmit = event => { event.preventDefault(); key = $("key").value.trim(); $("key").value = ""; connect(); };
window.addEventListener("hashchange", () => {
  const replacement = new URLSearchParams(location.hash.slice(1)).get("access");
  if (!replacement) return;
  stream?.abort(); clearFeed(); selected = null;
  $("sessions").replaceChildren();
  key = replacement; history.replaceState(null, "", location.pathname); connect();
});
function backToLive() {
  search.cancel(false, "Back to live · enter a query to search again");
  historyGeneration++;
  paused = false; historyOffset = 0; $("live").hidden = true; $("pause").textContent = "Pause updates";
  if (latestFrame) renderFrame(latestFrame);
  updateControl();
}
$("pause").onclick = () => {
  if (paused) { backToLive(); return; }
  paused = true; $("pause").textContent = "Resume updates";
  relinquish();
  $("feed-status").textContent = "Paused locally · terminal continues on host";
};
$("live").onclick = backToLive;
$("history").onclick = async () => {
  search.cancel(false);
  const id = selected, controller = stream;
  if (!id || !controller) return;
  const request = ++historyGeneration;
  paused = true; relinquish();
  const offset = Math.min(historyOffset + (latestFrame?.rows || 24), 100000);
  $("history").disabled = true;
  try {
    const response = await fetch("/sessions/" + encodeURIComponent(id) + "/history/" + offset, {headers:headers(), signal:controller.signal});
    if ([401,403,404].includes(response.status)) throw new AccessEnded("Session access ended.");
    if (!response.ok) throw new Error("History temporarily unavailable.");
    const frame = await response.json();
    if (controller.signal.aborted || selected !== id || request !== historyGeneration) return;
    historyOffset = offset; paused = true; renderFrame(frame);
    $("live").hidden = false; $("pause").textContent = "Resume updates";
    $("feed-status").textContent = "Retained history · " + offset + " rows before live output";
  } catch (error) {
    if (controller.signal.aborted || request !== historyGeneration) return;
    if (error instanceof AccessEnded) { controller.abort(); clearFeed(); }
    $("feed-status").textContent = error.message;
  }
  finally { if (selected === id && !controller.signal.aborted && latestFrame) $("history").disabled = false; }
};
$("copy").onclick = async () => {
  const selection = window.getSelection();
  const selectedText = selection && $("output").contains(selection.anchorNode) && $("output").contains(selection.focusNode) ? selection.toString() : "";
  try { await navigator.clipboard.writeText(selectedText || terminalDisplay.text); $("feed-status").textContent = "Copied to clipboard"; }
  catch { $("feed-status").textContent = "Select output and use your browser's Copy command."; }
};
$("side-view").onclick = () => { if (catalogReady && selected) sideViews.add(selected); };
$("search-form").onsubmit = event => { event.preventDefault(); search.start($("search-query").value); };
$("search-query").oninput = () => search.cancel(true, "Query changed; press Enter to search");
$("search-cancel").onclick = () => search.cancel(false, `Search cancelled · ${search.matches.length} partial results`);
$("claim").onclick = () => { control.send({type:"claim"}); $("claim").disabled = true; };
$("release").onclick = relinquish;
$("reattach").onclick = () => { if (selected) selectSession(selected); };
$("resize").onclick = () => control.send({type:"resize", columns:Number($("columns").value), rows:Number($("rows").value)});
const input = $("terminal-input");
function pasteText(text) {
  if (new TextEncoder().encode(text).length > 16384) { $("feed-status").textContent = "Paste is limited to 16 KiB."; return; }
  if (/[\x00-\x1f\x7f]/.test(text)) {
    $("paste-preview").value = text; $("paste-confirmation").hidden = false;
    $("paste-confirm").focus();
  } else control.send({type:"paste", text, confirmed:false});
}
bindTerminalInput(input, {sendKey:key=>control.send({type:"key",input:key}), pasteText, leaveInput:()=>$("release").focus()});
$("paste-confirm").onclick = () => {
  if (!input.disabled) control.send({type:"paste", text:$("paste-preview").value, confirmed:true});
  $("paste-preview").value = ""; $("paste-confirmation").hidden = true; input.focus();
};
$("paste-cancel").onclick = () => { $("paste-preview").value = ""; $("paste-confirmation").hidden = true; input.focus(); };
const heartbeat = setInterval(() => {
  if (control.lease && !control.pending && !control.queue.length) control.send({type:"heartbeat"});
}, 5000);
window.addEventListener("pagehide", () => { workflow.setEnabled(false); search.cancel(true); clearInterval(heartbeat); generation++; catalogReady = false; sideViews.clear(); sessionStream?.abort(); stream?.abort(); control.detach(); });
if (key) connect();
