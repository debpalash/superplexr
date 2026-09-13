// One ordered command lane per SSE attachment. It never retries input and
// never transfers queued work or a Control lease to a replacement attachment.
export class BrowserControl {
  constructor({headers, onChange, onFailure, fetcher = fetch}) {
    Object.assign(this, {headers, onChange, onFailure, fetcher});
    this.surface = null; this.lease = null; this.sequence = 0;
    this.queue = []; this.pending = false; this.generation = 0;
  }
  attach(surface) { this.detach(); this.surface = surface; this.onChange(this); }
  detach() {
    this.generation++; this.abort?.abort(); this.abort = null;
    this.surface = null; this.lease = null; this.sequence = 0;
    this.queue = []; this.pending = false; this.onChange(this);
  }
  send(action) {
    if (!this.surface || (action.type !== "claim" && !this.lease)) return false;
    if (this.queue.length >= 128) { this.fail("Input queue full. Reattach and check the terminal before continuing."); return false; }
    // Keep the lease at enqueue time: a queued key cannot gain future authority.
    this.queue.push({action, lease:this.lease});
    this.pump(); return true;
  }
  fail(message) { this.detach(); this.onFailure(message); }
  async pump() {
    if (this.pending || !this.queue.length || !this.surface) return;
    const current = this.generation, surface = this.surface;
    const request = this.queue.shift();
    request.sequence = ++this.sequence;
    this.pending = true;
    const abort = new AbortController(); this.abort = abort;
    const timer = setTimeout(() => abort.abort(), 10000);
    try {
      // Invoke fetch as a function, not a method on this client (browser brand check).
      const fetcher = this.fetcher;
      const response = await fetcher("/views/" + encodeURIComponent(surface) + "/commands", {
        method:"POST", headers:{...this.headers(), "Content-Type":"application/json"},
        body:JSON.stringify(request), signal:abort.signal, cache:"no-store"
      });
      if (!response.ok) throw new Error(response.status === 428 ? "Paste needs explicit confirmation." : "Control unavailable or changed. Reattach before sending more input.");
      const result = await response.json();
      if (current !== this.generation) return;
      if (result.sequence !== request.sequence) throw new Error("Command acknowledgement mismatch.");
      this.lease = result.lease;
      this.onChange(this);
    } catch (error) {
      if (current === this.generation) this.fail("Command was not confirmed; it will not be replayed. " + error.message);
    } finally {
      clearTimeout(timer);
      if (current === this.generation) { this.pending = false; this.abort = null; this.onChange(this); this.pump(); }
    }
  }
}

const keyNames = {Enter:"enter", Tab:"tab", Escape:"escape", Backspace:"backspace", Delete:"delete", Insert:"insert", ArrowLeft:"left", ArrowRight:"right", ArrowUp:"up", ArrowDown:"down", Home:"home", End:"end", PageUp:"pageup", PageDown:"pagedown", " ":"space"};

// DOM input is not always one key event: IMEs, mobile keyboards and accessibility
// drivers can insert text separately. This adapter sends each input once.
export function bindTerminalInput(input, {sendKey, pasteText, leaveInput}) {
  let forwardedKey = false;
  const handlers = {
    keydown(event) {
      forwardedKey = false;
      if (input.disabled) return;
      if (event.ctrlKey && event.key === "]") { event.preventDefault(); leaveInput(); return; }
      const key = terminalKey(event);
      if (key) { forwardedKey = true; event.preventDefault(); sendKey(key); }
    },
    beforeinput(event) { if (forwardedKey && !event.isComposing) event.preventDefault(); },
    keyup() { forwardedKey = false; },
    blur() { forwardedKey = false; },
    compositionstart() { forwardedKey = false; },
    compositionend(event) {
      if (!input.disabled && event.data) pasteText(event.data);
      input.value = "";
    },
    input(event) {
      if (event.isComposing || event.inputType === "insertCompositionText") return;
      if (!forwardedKey && !input.disabled && input.value) pasteText(input.value);
      input.value = "";
    },
    paste(event) {
      event.preventDefault();
      if (!input.disabled) pasteText(event.clipboardData?.getData("text/plain") || "");
    }
  };
  for (const [type,handler] of Object.entries(handlers)) input.addEventListener(type,handler);
  return () => { for (const [type,handler] of Object.entries(handlers)) input.removeEventListener(type,handler); };
}

export function terminalKey(event) {
  // Browser/OS shortcuts and clipboard handling remain local.
  if (event.isComposing || event.metaKey || (event.ctrlKey && event.shiftKey && ["C","V"].includes(event.key.toUpperCase()))) return null;
  const character = [...event.key].length === 1;
  const physical = keyNames[event.key] || (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(event.key) ? event.key.toLowerCase() : character ? event.key.toLowerCase() : null);
  if (!physical) return null;
  return {
    physical_key:physical, logical_key:event.key, text:character ? event.key : null,
    modifiers:{shift:event.shiftKey, alt:event.altKey, control:event.ctrlKey, super_key:false, caps_lock:!!event.getModifierState?.("CapsLock"), num_lock:false},
    consumed_modifiers:{shift:character && event.shiftKey, alt:false, control:false, super_key:false, caps_lock:false, num_lock:false},
    action:event.repeat ? "repeat" : "press", composing:false,
    unshifted_codepoint:character && [...event.key.toLowerCase()].length === 1 ? event.key.toLowerCase() : null
  };
}
