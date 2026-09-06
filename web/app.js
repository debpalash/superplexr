// The web shell. One WebSocket carries the same wire the desktop and TUI use;
// this file is the state machine around it: pairing, the session list, one
// open terminal with keys in and frames out, selection and copy.

import { Kind, FrameReader, Sequencer, encodeJson, hello, json, uuid, PROTOCOL_VERSION } from "/wire.js";
import { decodeFullFrame, decodeFrameDelta, decodeLifecycle, applyDelta } from "/frames.js";
import { Renderer, selectedText } from "/render.js";

const $ = (id) => document.getElementById(id);
const el = {
  link: $("link"),
  control: $("control"),
  newShell: $("new-shell"),
  replayToggle: $("replay-toggle"),
  notify: $("notify"),
  soft: $("soft"),
  keybar: $("keybar"),
  replay: $("replay"),
  forget: $("forget"),
  pair: $("pair"),
  pairForm: $("pair-form"),
  pairCode: $("pair-code"),
  pairError: $("pair-error"),
  main: $("main"),
  sessions: $("sessions"),
  screen: $("screen"),
  empty: $("empty"),
  status: $("status"),
};

// ---- what this browser remembers -------------------------------------------

const DEVICE_KEY = "ultraplexr.device";
const LAST_SESSION_KEY = "ultraplexr.last-session";

/** A viewer link carries a Share token after the hash; it never touches storage. */
function shareTokenFromLink() {
  const match = /(?:^#|[#&])share=([^&]+)/.exec(location.hash);
  return match ? decodeURIComponent(match[1]) : null;
}
const SHARE_TOKEN = shareTokenFromLink();

function loadDevice() {
  try {
    const raw = localStorage.getItem(DEVICE_KEY);
    if (raw) return JSON.parse(raw);
  } catch {}
  return { id: uuid(), token: null };
}

function saveDevice(device) {
  try { localStorage.setItem(DEVICE_KEY, JSON.stringify(device)); } catch {}
}

function forgetDevice() {
  try { localStorage.removeItem(DEVICE_KEY); localStorage.removeItem(LAST_SESSION_KEY); } catch {}
}

// ---- the connection --------------------------------------------------------

class Connection {
  constructor(device, { pairingCode = null, shareToken = null, onEvent, onState, onClose }) {
    this.device = device;
    this.pairingCode = pairingCode;
    this.shareToken = shareToken;
    this.onEvent = onEvent;
    this.onState = onState;
    this.onClose = onClose;
    this.clientId = uuid();
    this.reader = new FrameReader();
    this.sequencer = new Sequencer();
    this.pending = new Map();
    this.streams = new Map();
    this.welcome = null;
    this.refused = null;
    const scheme = location.protocol === "https:" ? "wss" : "ws";
    this.ws = new WebSocket(`${scheme}://${location.host}/ws`);
    this.ws.binaryType = "arraybuffer";
    this.ws.onopen = () => {
      const greeting = hello(this.clientId, device.id, { deviceToken: shareToken ? null : device.token, pairingCode });
      greeting.share_token = shareToken;
      this.send(encodeJson(Kind.Hello, 0, greeting, this.sequencer));
    };
    this.ws.onmessage = (event) => {
      this.reader.push(new Uint8Array(event.data));
      try {
        for (const frame of this.reader.take()) this.dispatch(frame);
      } catch (error) {
        console.error(error);
        this.ws.close(1002, String(error.message ?? error));
      }
    };
    this.ws.onclose = (event) => {
      for (const { reject } of this.pending.values()) reject(new Error("connection closed"));
      this.pending.clear();
      this.onClose({ refused: this.refused, code: event.code, reason: event.reason });
    };
    this.ws.onerror = () => {};
  }

  send(bytes) {
    if (this.ws.readyState === WebSocket.OPEN) this.ws.send(bytes);
  }

  dispatch(frame) {
    if (!this.welcome) {
      if (frame.kind === Kind.Welcome) {
        this.welcome = json(frame);
        if (this.welcome.device_token) {
          this.device.token = this.welcome.device_token;
          saveDevice(this.device);
        }
        this.onState("live", this.welcome);
      } else if (frame.kind === Kind.Close) {
        this.refused = json(frame);
      } else {
        throw new Error(`handshake answered with kind ${frame.kind}`);
      }
      return;
    }
    if (frame.streamId === 0) {
      if (frame.kind === Kind.Response) {
        const response = json(frame);
        const waiter = this.pending.get(response.request_id);
        if (!waiter) return;
        this.pending.delete(response.request_id);
        if (response.result.status === "success") waiter.resolve(response.result.body);
        else waiter.reject(Object.assign(new Error(response.result.message), { code: response.result.code }));
      } else if (frame.kind === Kind.Close) {
        this.refused = json(frame);
      } else if (frame.kind === Kind.EventBatch) {
        this.onEvent(json(frame));
      }
      return;
    }
    const handler = this.streams.get(frame.streamId);
    if (handler) handler(frame);
  }

  request(action, { surfaceId = null, controlEpoch = null, timeoutMs = 30000 } = {}) {
    const requestId = uuid();
    const envelope = {
      version: PROTOCOL_VERSION,
      client_id: this.clientId,
      request_id: requestId,
      surface_id: surfaceId,
      control_epoch: controlEpoch,
      share_token: this.shareToken,
      action,
    };
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(requestId);
        reject(new Error(`no answer to ${action.type} within ${timeoutMs / 1000}s`));
      }, timeoutMs);
      this.pending.set(requestId, {
        resolve: (body) => { clearTimeout(timer); resolve(body); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
      this.send(encodeJson(Kind.Request, 0, envelope, this.sequencer));
    });
  }

  close() {
    this.ws.close(1000, "done");
  }
}

// ---- keys ------------------------------------------------------------------

const NAMED_KEYS = {
  Enter: "enter", Backspace: "backspace", Delete: "delete", Tab: "tab", Escape: "escape",
  ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down",
  Home: "home", End: "end", Insert: "insert", PageUp: "pageup", PageDown: "pagedown", " ": "space",
};
const CODE_KEYS = {
  Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\", Semicolon: ";",
  Quote: "'", Comma: ",", Period: ".", Slash: "/", Backquote: "`", Space: "space",
};

function physicalFromCode(code) {
  if (/^Key[A-Z]$/.test(code)) return code[3].toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code[5];
  return CODE_KEYS[code] ?? null;
}

const NO_MODS = { shift: false, alt: false, control: false, super_key: false, caps_lock: false, num_lock: false };

function keyInput(e) {
  let physical = null;
  let text = null;
  if (NAMED_KEYS[e.key]) physical = NAMED_KEYS[e.key];
  else if (/^F([1-9]|1[0-9]|2[0-5])$/.test(e.key)) physical = e.key.toLowerCase();
  else if ([...e.key].length === 1) {
    physical = physicalFromCode(e.code) ?? e.key.toLowerCase();
    if (!e.ctrlKey && !e.altKey) text = e.key;
  } else return null;
  if (physical === "space" && !e.ctrlKey && !e.altKey) text = " ";
  const codepoint = physical === "space" ? " " : [...physical].length === 1 ? physical : null;
  return {
    physical_key: physical,
    logical_key: e.key,
    text,
    modifiers: { shift: e.shiftKey, alt: e.altKey, control: e.ctrlKey, super_key: e.metaKey, caps_lock: e.getModifierState?.("CapsLock") ?? false, num_lock: false },
    consumed_modifiers: NO_MODS,
    action: e.repeat ? "repeat" : "press",
    composing: false,
    unshifted_codepoint: codepoint,
  };
}

// ---- the app ---------------------------------------------------------------

const app = {
  device: loadDevice(),
  connection: null,
  reconnectDelay: 1000,
  sessions: [],
  listTimer: null,
  open: null, // { sessionId, surfaceId, controlEpoch, writable, streamId, frame, selection, dirty }
  renderer: new Renderer(el.screen),
  focused: false,
  share: null, // ShareSummary when this page was opened from a viewer link
  viewers: [],
  replay: null, // { sessionId, streamId, chapters, journalBytes, offset, speed, frame, live }
};

function setLink(state, text) {
  el.link.dataset.state = state;
  el.link.textContent = text;
}

function connect({ pairingCode = null } = {}) {
  if (app.connection) app.connection.close();
  setLink("connecting", pairingCode ? "pairing" : "connecting");
  app.connection = new Connection(app.device, {
    pairingCode,
    shareToken: SHARE_TOKEN,
    onState: async (state, welcome) => {
      if (state === "live") {
        app.reconnectDelay = 1000;
        setLink("live", `${location.host} · ${welcome.runtime_version}`);
        el.pair.hidden = true;
        el.pairError.hidden = true;
        el.main.hidden = false;
        el.newShell.hidden = !!SHARE_TOKEN;
        el.forget.hidden = !!SHARE_TOKEN;
        refreshNotifyButton();
        if (SHARE_TOKEN) {
          try {
            const identity = await app.connection.request({ type: "share_identity" });
            app.share = identity.share;
            setLink("live", `${location.host} · shared as “${app.share.label}” · ${app.share.role}`);
          } catch (error) {
            setStatus(`share: ${error.message}`);
          }
        }
        refreshSessions();
        clearInterval(app.listTimer);
        app.listTimer = setInterval(refreshSessions, 4000);
        watchSessions();
        const last = app.open?.sessionId ?? safeGet(LAST_SESSION_KEY);
        if (last) openSession(last).catch(() => {});
      }
    },
    onEvent: () => {},
    onClose: ({ refused, reason }) => {
      clearInterval(app.listTimer);
      if (refused && refused.code === "gateway_unauthorized" && SHARE_TOKEN) {
        showRefusedLink(refused.message);
        return;
      }
      if (refused && refused.code === "gateway_unauthorized") {
        app.device.token = null;
        saveDevice(app.device);
        showPairing(pairingCode ? refused.message : null);
        return;
      }
      if (refused && refused.code === "device_revoked") {
        app.device.token = null;
        saveDevice(app.device);
        showPairing("this browser's pairing was revoked on the runtime");
        return;
      }
      if (refused) {
        showPairing(`${refused.code}: ${refused.message}`);
        return;
      }
      setLink("lost", `reconnecting in ${Math.round(app.reconnectDelay / 1000)}s${reason ? ` · ${reason}` : ""}`);
      if (app.open) app.open.streamId = null;
      setTimeout(() => connect(), app.reconnectDelay);
      app.reconnectDelay = Math.min(app.reconnectDelay * 2, 15000);
    },
  });
}

function showPairing(error) {
  el.main.hidden = true;
  el.newShell.hidden = true;
  el.forget.hidden = true;
  el.pair.hidden = false;
  setLink("refused", error ? "refused" : "not paired");
  el.pairError.textContent = error ?? "";
  el.pairError.hidden = !error;
  el.pairCode.focus();
}

function showRefusedLink(message) {
  el.main.hidden = true;
  el.pair.hidden = false;
  el.pairForm.hidden = true;
  setLink("refused", "link refused");
  el.pairError.textContent = message ?? "this viewer link is invalid, expired, or revoked";
  el.pairError.hidden = false;
  el.pair.append(el.pairError);
}

function safeGet(key) {
  try { return localStorage.getItem(key); } catch { return null; }
}

// ---- sessions --------------------------------------------------------------

async function refreshSessions() {
  if (!app.connection?.welcome) return;
  try {
    const body = await app.connection.request({ type: "list_terminals", include_archived: false });
    let sessions = body.terminals ?? [];
    if (app.share?.session_ids?.length) sessions = sessions.filter((s) => app.share.session_ids.includes(s.session_id));
    app.sessions = sessions;
    renderSessions();
    if (app.open?.sessionId) refreshViewers();
    else if (SHARE_TOKEN && sessions.length && !app.open) openSession(sessions[0].session_id).catch(() => {});
  } catch (error) {
    console.warn("list_terminals", error);
  }
}

/** Every change to a session's summary — control, hands, offers — as it happens. */
async function watchSessions() {
  try {
    const accepted = await app.connection.request({ type: "subscribe_terminals" });
    app.connection.streams.set(accepted.stream_id, (frame) => {
      if (frame.kind !== Kind.EventBatch) return;
      const event = json(frame);
      if (event.type !== "terminal_changed") return;
      const terminal = event.terminal;
      if (app.share?.session_ids?.length && !app.share.session_ids.includes(terminal.session_id)) return;
      const index = app.sessions.findIndex((s) => s.session_id === terminal.session_id);
      if (index >= 0) app.sessions[index] = terminal; else app.sessions.push(terminal);
      renderSessions();
      if (app.open?.sessionId === terminal.session_id) onSummary(terminal);
    });
  } catch (error) {
    console.warn("subscribe_terminals", error);
  }
}

function me() {
  return { client_id: app.connection?.clientId, surface_id: app.open?.surfaceId ?? null, share_id: app.share?.share_id ?? null };
}

function sameIdentity(a, b) {
  return !!a && !!b && a.client_id === b.client_id && (a.surface_id ?? null) === (b.surface_id ?? null) && (a.share_id ?? null) === (b.share_id ?? null);
}

/** The room's control state for the open session, from its summary. */
function onSummary(summary) {
  const open = app.open;
  if (!open) return;
  const mine = summary.controller_client_id === app.connection.clientId && (summary.controller_surface_id ?? null) === open.surfaceId;
  if (mine) open.controlEpoch = summary.control_epoch;
  if (open.writable !== mine) {
    open.writable = mine;
    if (mine) fitSession();
    schedulePaint();
  }
  open.summary = summary;
  updateControlBadge();
}

function sessionName(s) {
  const exe = s.foreground_process?.executable;
  if (exe) return exe.split("/").pop();
  return s.status === "running" ? "shell" : s.status;
}

function shortCwd(cwd) {
  if (!cwd) return "";
  return cwd.replace(/^\/Users\/[^/]+|^\/home\/[^/]+/, "~");
}

function renderSessions() {
  const rank = { running: 0, exited: 1, failed: 2 };
  const sessions = [...app.sessions].sort((a, b) => (rank[a.status] ?? 3) - (rank[b.status] ?? 3));
  el.sessions.replaceChildren(...sessions.map((s) => {
    const button = document.createElement("button");
    button.className = "session";
    button.dataset.status = s.status;
    button.setAttribute("aria-current", String(app.open?.sessionId === s.session_id));
    const dot = document.createElement("span");
    dot.className = "dot";
    const text = document.createElement("span");
    const name = document.createElement("div");
    name.className = "name";
    name.textContent = sessionName(s);
    const meta = document.createElement("div");
    meta.className = "meta";
    meta.textContent = `${shortCwd(s.cwd)}${s.controller_client_id ? " · held" : ""}`;
    text.append(name, meta);
    button.append(dot, text);
    button.onclick = () => openSession(s.session_id).catch((error) => setStatus(String(error.message ?? error)));
    return button;
  }));
}

async function refreshViewers() {
  const open = app.open;
  if (!open || !app.connection?.welcome) return;
  try {
    const body = await app.connection.request({ type: "terminal_viewers", session_id: open.sessionId });
    if (app.open === open) {
      app.viewers = body.viewers ?? [];
      schedulePaint();
    }
  } catch {}
}

function viewersText() {
  const n = app.viewers.length;
  if (!n) return "";
  const shares = app.viewers.filter((v) => v.kind === "share").length;
  return `👁 ${n}${shares ? ` (${shares} via link)` : ""}`;
}

function setStatus(text) {
  el.status.textContent = text;
}

function button(text, onClick, quiet = false) {
  const b = document.createElement("button");
  b.className = quiet ? "button quiet" : "button";
  b.textContent = text;
  b.onclick = onClick;
  return b;
}

function controlRequest(action, extra = {}) {
  const open = app.open;
  return app.connection.request({ type: action, session_id: open.sessionId, ...extra }, { surfaceId: open.surfaceId, controlEpoch: open.controlEpoch })
    .catch((error) => setStatus(`${action}: ${error.message}`));
}

/** Control is handed, never seized, except by the owner. The panel shows
 *  exactly the moves open to this participant right now. */
function updateControlBadge() {
  const open = app.open;
  if (!open) { el.control.hidden = true; return; }
  el.control.hidden = false;
  el.control.dataset.writable = String(open.writable);
  const summary = open.summary ?? {};
  const hands = summary.control_requests ?? [];
  const offer = summary.control_offer ?? null;
  const i = me();
  const canHold = !SHARE_TOKEN || app.share?.role === "controller";
  const children = [];
  if (open.writable) {
    children.push(document.createTextNode(offer ? `offering to ${offer.to?.label ?? "anyone"}` : "in control"));
    if (offer) children.push(button("withdraw", () => controlRequest("withdraw_terminal_control"), true));
    for (const hand of hands) children.push(button(`✋ hand to ${hand.label}`, () => controlRequest("offer_terminal_control", { to: hand })));
    if (!offer && !hands.length) children.push(button("offer to anyone", () => controlRequest("offer_terminal_control", { to: null }), true));
    children.push(button("release", () => controlRequest("release_terminal_control"), true));
  } else {
    const offeredToMe = offer && (!offer.to || sameIdentity(offer.to, i));
    const raised = hands.some((hand) => sameIdentity(hand, i));
    const holder = summary.controller_client_id ? "watching" : "nobody holds control";
    children.push(document.createTextNode(holder));
    if (canHold && offeredToMe) children.push(button("accept control", async () => {
      const body = await controlRequest("accept_terminal_control");
      if (body?.control_epoch !== undefined) { open.controlEpoch = body.control_epoch; open.writable = true; fitSession(); updateControlBadge(); }
    }));
    else if (canHold && raised) children.push(button("✋ raised · lower", () => controlRequest("withdraw_terminal_control"), true));
    else if (canHold && summary.controller_client_id) children.push(button("✋ raise hand", () => controlRequest("request_terminal_control")));
    else if (canHold) children.push(button("take control", () => claimControl(false)));
    if (!SHARE_TOKEN && summary.controller_client_id) children.push(button("take", () => claimControl(true), true));
    if (hands.length) { const h = document.createElement("span"); h.className = "hands"; h.textContent = `✋ ${hands.length}`; children.push(h); }
  }
  el.control.replaceChildren(...children);
}

async function claimControl(force) {
  const open = app.open;
  if (!open) return;
  try {
    const body = await app.connection.request(
      { type: "claim_terminal_control", session_id: open.sessionId, force },
      { surfaceId: open.surfaceId, controlEpoch: open.controlEpoch },
    );
    open.controlEpoch = body.control_epoch;
    open.writable = true;
    fitSession();
  } catch (error) {
    open.writable = false;
    if (force) setStatus(`could not take control: ${error.message}`);
  }
  open.summary = app.sessions.find((s) => s.session_id === open.sessionId) ?? open.summary;
  updateControlBadge();
}

async function openSession(sessionId, { surfaceId = null } = {}) {
  const connection = app.connection;
  if (!connection?.welcome) return;
  if (app.open?.streamId) {
    const previous = app.open;
    connection.streams.delete(previous.streamId);
    connection.request({ type: "unsubscribe", stream_id: previous.streamId }).catch(() => {});
  }
  const open = {
    sessionId,
    surfaceId: surfaceId ?? (app.open?.sessionId === sessionId ? app.open.surfaceId : uuid()),
    controlEpoch: app.open?.sessionId === sessionId ? app.open.controlEpoch : null,
    writable: false,
    streamId: null,
    frame: null,
    selection: null,
    dirty: false,
    summary: app.sessions.find((s) => s.session_id === sessionId) ?? null,
  };
  app.open = open;
  try { localStorage.setItem(LAST_SESSION_KEY, sessionId); } catch {}
  renderSessions();
  const snapshot = await connection.request({ type: "terminal_snapshot", session_id: sessionId });
  if (app.open !== open) return;
  open.frame = snapshot.frame;
  el.empty.hidden = true;
  schedulePaint();
  await claimControl(false);
  const accepted = await connection.request({ type: "subscribe_terminal", session_id: sessionId, max_hz: 60 });
  if (app.open !== open) {
    connection.request({ type: "unsubscribe", stream_id: accepted.stream_id }).catch(() => {});
    return;
  }
  open.streamId = accepted.stream_id;
  connection.streams.set(accepted.stream_id, (frame) => onStreamFrame(open, frame));
  refreshViewers();
  el.replayToggle.hidden = false;
  if (app.replay) stopReplay();
  el.screen.focus();
}

function onStreamFrame(open, frame) {
  if (app.open !== open) return;
  switch (frame.kind) {
    case Kind.FullFrame: {
      open.frame = decodeFullFrame(frame.payload).frame;
      schedulePaint();
      break;
    }
    case Kind.FrameDelta: {
      const { delta } = decodeFrameDelta(frame.payload);
      if (!open.frame) return;
      try {
        applyDelta(open.frame, delta);
      } catch (error) {
        console.warn("delta rejected, taking a snapshot", error);
        app.connection.request({ type: "terminal_snapshot", session_id: open.sessionId })
          .then((body) => { if (app.open === open) { open.frame = body.frame; schedulePaint(); } })
          .catch(() => {});
        return;
      }
      schedulePaint();
      break;
    }
    case Kind.TerminalLifecycle: {
      const life = decodeLifecycle(frame.payload);
      if (life.state === "exited") {
        const how = life.exit?.code !== undefined ? `exit ${life.exit.code}` : life.exit?.signal !== undefined ? `signal ${life.exit.signal}` : "exited";
        setStatus(`session ended · ${how}`);
      } else if (life.state === "lost") {
        setStatus(`session lost${life.reason ? ` · ${life.reason}` : ""}`);
      }
      refreshSessions();
      break;
    }
    case Kind.ResyncRequired: {
      app.connection.request({ type: "terminal_snapshot", session_id: open.sessionId })
        .then((body) => { if (app.open === open) { open.frame = body.frame; schedulePaint(); } })
        .catch(() => {});
      break;
    }
    case Kind.Close: {
      open.streamId = null;
      break;
    }
    default:
      break;
  }
}

// ---- replay ----------------------------------------------------------------
// A recording plays into the same canvas as the live session; the live
// stream keeps its frames in the background and is shown again on stop.

async function startReplay(fromOffset = 0) {
  const open = app.open;
  if (!open) return;
  if (app.replay) await stopReplay({ keepPanel: true });
  const speed = app.replay?.speed ?? Number(el.replay.querySelector("select")?.value ?? 1);
  let chapters = app.replay?.chapters;
  let journalBytes = app.replay?.journalBytes ?? 0;
  if (!chapters) {
    const body = await app.connection.request({ type: "terminal_chapters", session_id: open.sessionId });
    chapters = body.chapters;
    journalBytes = body.journal_bytes;
  }
  const replay = { sessionId: open.sessionId, streamId: null, chapters, journalBytes, offset: fromOffset, speed, frame: null, ended: false };
  app.replay = replay;
  renderReplayPanel();
  try {
    const accepted = await app.connection.request({ type: "subscribe_terminal_replay", session_id: open.sessionId, from_offset: fromOffset, speed_percent: Math.round(speed * 100), max_hz: 30 });
    if (app.replay !== replay) return;
    replay.streamId = accepted.stream_id;
    app.connection.streams.set(accepted.stream_id, (frame) => {
      if (app.replay !== replay) return;
      if (frame.kind === Kind.FullFrame) replay.frame = decodeFullFrame(frame.payload).frame;
      else if (frame.kind === Kind.FrameDelta && replay.frame) { try { applyDelta(replay.frame, decodeFrameDelta(frame.payload).delta); } catch { replay.frame = null; } }
      else if (frame.kind === Kind.Close) { replay.ended = true; renderReplayPanel(); }
      schedulePaint();
    });
  } catch (error) {
    setStatus(`replay: ${error.message}`);
    app.replay = null;
    renderReplayPanel();
  }
}

async function stopReplay({ keepPanel = false } = {}) {
  const replay = app.replay;
  if (!replay) return;
  if (replay.streamId) {
    app.connection.streams.delete(replay.streamId);
    app.connection.request({ type: "unsubscribe", stream_id: replay.streamId }).catch(() => {});
  }
  app.replay = null;
  if (!keepPanel) el.replay.hidden = true;
  schedulePaint();
}

function renderReplayPanel() {
  const replay = app.replay;
  el.replay.hidden = !replay;
  if (!replay) return;
  const children = [];
  children.push(button("■ live", () => stopReplay(), true));
  const speed = document.createElement("select");
  for (const s of [0.5, 1, 2, 4, 8, 16]) {
    const o = document.createElement("option");
    o.value = String(s); o.textContent = `×${s}`; o.selected = s === replay.speed;
    speed.append(o);
  }
  speed.onchange = () => { replay.speed = Number(speed.value); startReplay(replay.offset); };
  children.push(speed);
  const bar = document.createElement("div");
  bar.className = "bar";
  const fill = document.createElement("div");
  fill.style.width = replay.journalBytes ? `${Math.min(100, (100 * replay.offset) / replay.journalBytes).toFixed(1)}%` : "0";
  bar.append(fill);
  children.push(bar);
  for (const c of replay.chapters) {
    const b = button(`${c.index} · ${c.label}`, () => startReplay(c.journal_offset), true);
    b.classList.add("chapter");
    b.dataset.kind = c.kind;
    b.title = `${c.kind} · offset ${c.journal_offset}`;
    children.push(b);
  }
  if (replay.ended) { const e = document.createElement("span"); e.textContent = "· end of recording"; children.push(e); }
  el.replay.replaceChildren(...children);
}

el.replayToggle.onclick = () => { if (app.replay) stopReplay(); else startReplay(0); };

// ---- installable, and told when something needs you ------------------------

const TOUCH = matchMedia("(pointer: coarse)").matches;

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch((error) => console.warn("service worker", error));
}

function urlBase64ToBytes(text) {
  const padded = text.replace(/-/g, "+").replace(/_/g, "/") + "=".repeat((4 - (text.length % 4)) % 4);
  return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
}

async function refreshNotifyButton() {
  if (SHARE_TOKEN || !("PushManager" in window) || !("Notification" in window)) { el.notify.hidden = true; return; }
  el.notify.hidden = false;
  const registration = await navigator.serviceWorker?.ready;
  const current = await registration?.pushManager.getSubscription();
  el.notify.textContent = current ? "🔔 on" : "🔔 notify";
  el.notify.dataset.on = String(!!current);
}

el.notify.onclick = async () => {
  try {
    const registration = await navigator.serviceWorker.ready;
    const current = await registration.pushManager.getSubscription();
    if (current) {
      await app.connection.request({ type: "forget_push_subscription", endpoint: current.endpoint });
      await current.unsubscribe();
      setStatus("notifications off");
    } else {
      if ((await Notification.requestPermission()) !== "granted") { setStatus("notifications were not allowed"); return; }
      const info = await app.connection.request({ type: "push_info" });
      if (!info.public_key) { setStatus("this runtime has push turned off"); return; }
      const subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: urlBase64ToBytes(info.public_key) });
      const json = subscription.toJSON();
      await app.connection.request({ type: "register_push_subscription", subscription: { endpoint: subscription.endpoint, p256dh: json.keys.p256dh, auth: json.keys.auth, label: navigator.userAgent.slice(0, 60) } });
      setStatus("notifications on: Faults, approvals and finished Runs reach this device");
    }
  } catch (error) {
    setStatus(`notifications: ${error.message}`);
  }
  refreshNotifyButton();
};

// ---- phones: a soft keyboard and the keys it lacks ------------------------

const held = { control: false, alt: false };

function sendKeyName(name, { ctrl = false, alt = false, text = null } = {}) {
  const open = app.open;
  if (!open?.writable) return;
  const input = {
    physical_key: name, logical_key: name, text,
    modifiers: { shift: false, alt, control: ctrl, super_key: false, caps_lock: false, num_lock: false },
    consumed_modifiers: NO_MODS, action: "press", composing: false,
    unshifted_codepoint: [...name].length === 1 ? name : null,
  };
  app.connection.request({ type: "terminal_key", session_id: open.sessionId, input }, { surfaceId: open.surfaceId, controlEpoch: open.controlEpoch, timeoutMs: 10000 })
    .catch((error) => setStatus(`key: ${error.message}`));
}

if (TOUCH) {
  el.keybar.hidden = false;
  el.screen.addEventListener("pointerup", () => { el.soft.focus({ preventScroll: true }); });
  el.soft.addEventListener("focus", () => { app.focused = true; schedulePaint(); });
  el.soft.addEventListener("blur", () => { app.focused = false; schedulePaint(); });
  el.soft.addEventListener("keydown", (e) => {
    // Hardware keyboards on tablets come through here too.
    if (e.key === "Unidentified" || e.isComposing) return;
    const input = keyInput(e);
    if (!input) return;
    e.preventDefault();
    if (held.control || held.alt) { input.modifiers.control = input.modifiers.control || held.control; input.modifiers.alt = input.modifiers.alt || held.alt; if (held.control) input.text = null; releaseHeld(); }
    const open = app.open;
    if (!open?.writable) return;
    app.connection.request({ type: "terminal_key", session_id: open.sessionId, input }, { surfaceId: open.surfaceId, controlEpoch: open.controlEpoch, timeoutMs: 10000 }).catch(() => {});
  });
  el.soft.addEventListener("input", () => {
    // Text the soft keyboard composed (autocorrect, swipe, emoji) that keydown did not carry.
    const text = el.soft.value;
    el.soft.value = "";
    if (!text) return;
    for (const ch of text) {
      if (ch === "\n") sendKeyName("enter");
      else sendKeyName(ch === " " ? "space" : ch.toLowerCase(), { ctrl: held.control, alt: held.alt, text: held.control ? null : ch });
    }
    releaseHeld();
  });
  for (const b of el.keybar.querySelectorAll("button")) {
    b.addEventListener("pointerdown", (e) => e.preventDefault());
    b.onclick = () => {
      if (b.dataset.mod) { held[b.dataset.mod] = !held[b.dataset.mod]; b.setAttribute("aria-pressed", String(held[b.dataset.mod])); return; }
      const name = b.dataset.key;
      const ctrl = b.dataset.ctrl === "1" || held.control;
      sendKeyName(name, { ctrl, alt: held.alt, text: !ctrl && [...name].length === 1 ? name : null });
      releaseHeld();
      el.soft.focus({ preventScroll: true });
    };
  }
  visualViewport?.addEventListener("resize", () => { app.renderer.measure(); schedulePaint(); clearTimeout(resizeTimer); resizeTimer = setTimeout(fitSession, 150); });
}

function releaseHeld() {
  held.control = false; held.alt = false;
  for (const b of el.keybar.querySelectorAll("button[data-mod]")) b.setAttribute("aria-pressed", "false");
}

// ---- painting --------------------------------------------------------------

function schedulePaint() {
  if (!app.open || app.open.dirty) return;
  app.open.dirty = true;
  requestAnimationFrame(() => {
    const open = app.open;
    if (!open) return;
    open.dirty = false;
    const replay = app.replay;
    if (replay?.frame) {
      app.renderer.paint(replay.frame, { focused: false, selection: null });
      setStatus(`▶ replay ×${replay.speed} · ${replay.frame.title ?? ""}${replay.ended ? " · ended" : ""}`);
      return;
    }
    if (!open.frame) return;
    app.renderer.paint(open.frame, { focused: app.focused, selection: open.selection });
    const f = open.frame;
    const parts = [];
    if (f.title) parts.push(f.title);
    if (f.current_directory) parts.push(shortCwd(f.current_directory));
    parts.push(`${f.grid.columns}×${f.grid.rows}`);
    parts.push(open.writable ? "control" : "watching");
    const eyes = viewersText();
    if (eyes) parts.push(eyes);
    if (f.mouse_tracking) parts.push("mouse");
    setStatus(parts.join(" · "));
  });
}

// ---- size ------------------------------------------------------------------

let resizeTimer = null;

function fitSession() {
  const open = app.open;
  if (!open?.writable || !open.frame) return;
  const fit = app.renderer.fit();
  if (fit.columns === open.frame.grid.columns && fit.rows === open.frame.grid.rows) return;
  app.connection.request(
    { type: "terminal_resize", session_id: open.sessionId, grid: { columns: fit.columns, rows: fit.rows }, cell_width_px: fit.cellWidthPx, cell_height_px: fit.cellHeightPx },
    { surfaceId: open.surfaceId, controlEpoch: open.controlEpoch },
  ).catch((error) => setStatus(`resize: ${error.message}`));
}

new ResizeObserver(() => {
  app.renderer.measure();
  schedulePaint();
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(fitSession, 150);
}).observe(el.screen);

// ---- input -----------------------------------------------------------------

el.screen.addEventListener("focus", () => { app.focused = true; schedulePaint(); });
el.screen.addEventListener("blur", () => { app.focused = false; schedulePaint(); });

el.screen.addEventListener("keydown", (e) => {
  const open = app.open;
  if (!open) return;
  if (app.replay) { if (e.key === "Escape") stopReplay(); return; }
  if (e.metaKey || e.isComposing) return; // the browser's own shortcuts, copy and paste included
  const input = keyInput(e);
  if (!input) return;
  e.preventDefault();
  if (!open.writable) return;
  open.selection = null;
  app.connection.request({ type: "terminal_key", session_id: open.sessionId, input }, { surfaceId: open.surfaceId, controlEpoch: open.controlEpoch, timeoutMs: 10000 })
    .catch((error) => {
      if (error.code === "control_lost" || /control/.test(error.message ?? "")) {
        open.writable = false;
        updateControlBadge();
      }
      setStatus(`key: ${error.message}`);
    });
});

document.addEventListener("paste", async (e) => {
  const open = app.open;
  if (!open?.writable || document.activeElement !== el.screen) return;
  const text = e.clipboardData?.getData("text/plain");
  if (!text) return;
  e.preventDefault();
  const bytes = [...new TextEncoder().encode(text)];
  const paste = (confirmed) => app.connection.request({ type: "terminal_paste", session_id: open.sessionId, bytes, confirmed }, { surfaceId: open.surfaceId, controlEpoch: open.controlEpoch });
  try {
    await paste(false);
  } catch (error) {
    if (/confirm|unsafe|multiline|escape/i.test(`${error.code} ${error.message}`) && window.confirm("The clipboard holds line breaks or control characters. Paste it anyway?")) {
      await paste(true).catch((again) => setStatus(`paste: ${again.message}`));
    } else {
      setStatus(`paste: ${error.message}`);
    }
  }
});

document.addEventListener("copy", (e) => {
  const open = app.open;
  if (!open?.selection || !open.frame || document.activeElement !== el.screen) return;
  const text = selectedText(open.frame, open.selection);
  if (!text) return;
  e.clipboardData.setData("text/plain", text);
  e.preventDefault();
});

let dragging = false;
el.screen.addEventListener("pointerdown", (e) => {
  const open = app.open;
  if (!open?.frame || e.button !== 0) return;
  el.screen.focus();
  const rect = el.screen.getBoundingClientRect();
  const at = app.renderer.cellAt(e.clientX - rect.left, e.clientY - rect.top);
  open.selection = { anchor: at, head: at };
  dragging = true;
  el.screen.setPointerCapture(e.pointerId);
  schedulePaint();
});
el.screen.addEventListener("pointermove", (e) => {
  const open = app.open;
  if (!dragging || !open?.selection) return;
  const rect = el.screen.getBoundingClientRect();
  const at = app.renderer.cellAt(e.clientX - rect.left, e.clientY - rect.top);
  at.column = Math.max(0, Math.min(at.column, open.frame.grid.columns - 1));
  at.row = Math.max(0, Math.min(at.row, open.frame.grid.rows - 1));
  open.selection.head = at;
  schedulePaint();
});
el.screen.addEventListener("pointerup", () => {
  dragging = false;
  const open = app.open;
  if (open?.selection && open.selection.anchor.row === open.selection.head.row && open.selection.anchor.column === open.selection.head.column) {
    open.selection = null;
    schedulePaint();
  }
});

// ---- chrome ----------------------------------------------------------------

el.pairForm.addEventListener("submit", (e) => {
  e.preventDefault();
  const code = el.pairCode.value.trim().toUpperCase().replace(/[^A-Z0-9]/g, "");
  if (!code) return;
  connect({ pairingCode: code });
});

el.newShell.onclick = async () => {
  const fit = app.renderer.fit();
  // The surface that starts a session holds its control; open it as the same one.
  const surfaceId = uuid();
  try {
    const body = await app.connection.request({
      type: "start_terminal",
      spec: { session_id: uuid(), mission_id: null, run_id: null, program: "", args: [], cwd: "", environment_delta: {}, grid: { columns: fit.columns, rows: fit.rows } },
    }, { surfaceId, timeoutMs: 20000 });
    await refreshSessions();
    await openSession(body.terminal.session_id, { surfaceId });
  } catch (error) {
    setStatus(`new shell: ${error.message}`);
  }
};

el.forget.onclick = () => {
  forgetDevice();
  app.connection?.close();
  app.device = { id: uuid(), token: null };
  showPairing(null);
};

if (SHARE_TOKEN || app.device.token) connect();
else showPairing(null);
