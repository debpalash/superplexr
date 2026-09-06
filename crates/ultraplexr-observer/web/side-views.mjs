import {AccessEnded, followFeed} from "/stream.mjs";
import {FrameContinuityError, FrameDecoder} from "/frames.mjs";
import {TerminalDisplay} from "/terminal.mjs";

const element = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
};
const button = (label, action) => {
  const node = element("button", "", label); node.type = "button"; node.onclick = action; return node;
};

class SideView {
  constructor(session, {headers, close, openMain}) {
    this.session = session; this.headers = headers; this.controller = null;
    this.latest = null; this.paused = false; this.decoder = new FrameDecoder();
    this.root = element("section", "side-view"); this.root.setAttribute("aria-label", "Read-only terminal view");
    const heading = element("div", "side-view-heading");
    this.title = element("h2", "", session.slice(0, 8));
    const badge = element("span", "scope", "Read-only view");
    this.directory = element("p", "side-view-directory");
    this.status = element("p", "side-view-status", "Connecting…"); this.status.setAttribute("role", "status");
    this.output = element("pre", "side-view-output"); this.output.tabIndex = 0;
    this.output.setAttribute("aria-label", "Read-only terminal output");
    this.display = new TerminalDisplay(this.output);
    const controls = element("div", "controls");
    this.pause = button("Pause updates", () => {
      this.paused = !this.paused; this.pause.textContent = this.paused ? "Resume updates" : "Pause updates";
      this.output.classList.toggle("terminal-cursor-hidden", this.paused);
      if (!this.paused && this.latest) this.render(this.latest);
      else this.status.textContent = "Paused locally · terminal continues on host";
    });
    this.copy = button("Copy output", async () => {
      const selection = window.getSelection();
      const selected = selection && this.output.contains(selection.anchorNode) && this.output.contains(selection.focusNode)
        ? selection.toString() : "";
      try { await navigator.clipboard.writeText(selected || this.display.text); this.status.textContent = "Copied to clipboard"; }
      catch { this.status.textContent = "Select output and use your browser's Copy command."; }
    });
    this.retry = button("Reconnect view", () => this.start()); this.retry.hidden = true;
    controls.append(this.pause, this.copy, this.retry, button("Open in main", () => openMain(this.session)), button("Close view", close));
    heading.append(this.title, badge);
    this.root.append(heading, this.directory, controls, this.status, this.output);
    this.pause.disabled = true; this.copy.disabled = true;
  }
  render(frame) {
    this.title.textContent = frame.title || this.session.slice(0, 8);
    this.directory.textContent = frame.directory || "";
    this.display.render(frame, {cursor:frame.status === "running"});
    this.pause.disabled = false; this.copy.disabled = false;
    this.status.textContent = `${frame.status} · read-only · frame ${frame.revision || frame.sequence}`;
  }
  async start() {
    this.controller?.abort(); this.decoder.reset(); this.latest = null; this.paused = false;
    this.display.clear(); this.directory.textContent = "";
    this.output.classList.remove("terminal-cursor-hidden");
    this.pause.textContent = "Pause updates"; this.pause.disabled = true; this.copy.disabled = true;
    this.retry.hidden = true; this.status.textContent = "Connecting read-only view…";
    const controller = new AbortController(); this.controller = controller;
    const headers = {...this.headers(), "X-Ultraplexr-Frames":"row-delta-v1", "X-Ultraplexr-View":"observe"};
    try {
      await followFeed({url:`/sessions/${encodeURIComponent(this.session)}/events`, headers, signal:controller.signal,
        onRetry:(error, ms) => {
          this.decoder.reset(); this.latest = null;
          if (error instanceof FrameContinuityError) {
            if (!headers["X-Ultraplexr-Frames"]) throw new AccessEnded("Invalid terminal frames; reconnect explicitly.");
            delete headers["X-Ultraplexr-Frames"];
          }
          this.pause.disabled = true;
          this.status.textContent = `Disconnected · read-only reconnect in ${ms / 1000}s`;
        },
        onEvent:({event, data}) => {
          if (controller.signal.aborted) return;
          if (event === "surface") {
            // An older gateway may ignore the observe-only header. Never use
            // its input identity or heartbeat; require a compatible gateway.
            throw new AccessEnded("Gateway did not honor read-only view mode; update it before opening side views.");
          }
          if (event === "frame" || event === "frame-delta") {
            this.latest = this.decoder.accept(event, data);
            this.pause.disabled = false;
            if (!this.paused) this.render(this.latest);
          } else if (event === "reconnecting") {
            this.decoder.reset(); this.latest = null; this.pause.disabled = true;
            this.status.textContent = "Runtime reconnecting · read-only view";
          } else if (event === "complete") {
            this.status.textContent = this.paused ? "Session ended · paused output retained locally" : "Session ended · final output retained locally";
          }
        },
      });
    } catch (error) {
      if (controller.signal.aborted || this.controller !== controller) return;
      this.decoder.reset(); this.latest = null; this.display.clear(); this.directory.textContent = "";
      this.title.textContent = this.session.slice(0, 8);
      this.pause.disabled = true; this.copy.disabled = true;
      this.status.textContent = error.message; this.retry.hidden = false;
    }
  }
  dispose() {
    this.controller?.abort(); this.decoder.reset(); this.latest = null;
    this.display.clear(); this.root.remove();
  }
}

export class SideViews {
  constructor(root, {headers, onChange, openMain}) {
    this.root = root; this.headers = headers; this.onChange = onChange; this.openMain = openMain;
    this.views = new Set();
  }
  get available() { return this.views.size < 3; }
  add(session) {
    if (!this.available || !session) return false;
    const view = new SideView(session, {headers:this.headers, openMain:this.openMain, close:() => this.remove(view)});
    this.views.add(view); this.root.append(view.root); this.changed(); view.start();
    return true;
  }
  remove(view) {
    if (!this.views.delete(view)) return;
    view.dispose(); this.changed();
  }
  reconcile(terminals) {
    const ids = new Set(terminals.map(terminal => terminal.session_id));
    for (const view of this.views) if (!ids.has(view.session)) this.remove(view);
  }
  clear() {
    for (const view of this.views) view.dispose();
    this.views.clear(); this.changed();
  }
  changed() {
    this.root.hidden = !this.views.size;
    this.root.parentElement.classList.toggle("has-side-views", !!this.views.size);
    this.onChange();
  }
}
