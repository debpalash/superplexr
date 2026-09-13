import {AccessEnded, readEvents} from "./stream.mjs";

const encoder = new TextEncoder();
export const MAX_QUERY_BYTES = 1024, MAX_RESULTS = 1000, MAX_PREVIEW_BYTES = 2 * 1024 * 1024;

async function boundedJson(response) {
  if (!response.body) throw new Error("History response is missing");
  const reader = response.body.getReader(), decoder = new TextDecoder();
  let text = "", bytes = 0;
  try {
    while (true) {
      const {value, done} = await reader.read();
      if (done) break;
      bytes += value.byteLength;
      if (bytes > 8 * 1024 * 1024) throw new Error("History response exceeds 8 MiB");
      text += decoder.decode(value, {stream:true});
    }
    return JSON.parse(text + decoder.decode());
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}

function checkResponse(response) {
  if ([401, 403, 404].includes(response.status)) throw new AccessEnded("Session access ended.");
  if (response.status === 429) throw new Error("Search capacity is busy; try again shortly");
  if (!response.ok) throw new Error("Search/history unavailable (" + response.status + ")");
}

// A query is bound to one HTTP request/session. No retry, persistence or input
// authority. Generation checks cover fetchers that deliver after cancellation.
export class BrowserSearch {
  constructor({headers, onChange = () => {}, onInspect = () => {}, onFrame = () => {}, onEnded = () => {}, fetcher = (...args) => globalThis.fetch(...args)}) {
    this.headers = headers; this.onChange = onChange; this.onInspect = onInspect;
    this.onFrame = onFrame; this.onEnded = onEnded;
    this.fetcher = (url, options) => fetcher(url, options);
    this.session = null; this.matches = []; this.running = false;
    this.status = "Choose a session to search its retained history";
    this.generation = 0; this.controller = null;
  }
  cancel(clear = false, status = "Search cancelled") {
    this.generation++; this.controller?.abort(); this.controller = null;
    this.running = false;
    if (clear) this.matches = [];
    this.status = status;
    this.onChange(this);
  }
  attach(session) {
    this.session = session;
    this.cancel(true, session ? "Enter a literal query to search retained history" : "Choose a session to search its retained history");
  }
  begin(status) {
    const controller = new AbortController(); this.controller = controller; this.running = true; this.status = status;
    const generation = this.generation, session = this.session;
    const timer = setTimeout(() => controller.abort(new Error("Search/history timed out")), 30000);
    this.onChange(this);
    return {controller, session, current: () => generation === this.generation && session === this.session, finish: () => clearTimeout(timer)};
  }
  failed(error, request) {
    if (!request.current()) return;
    this.matches = []; this.running = false;
    this.status = request.controller.signal.reason?.message || error.message || "Search failed";
    this.onChange(this);
    if (error instanceof AccessEnded) this.onEnded(error);
  }
  async start(query) {
    this.cancel(true);
    if (!this.session || !query || encoder.encode(query).length > MAX_QUERY_BYTES) {
      this.status = "Choose a session and enter a query of 1–1024 UTF-8 bytes"; this.onChange(this); return;
    }
    const request = this.begin("Searching retained history…");
    let complete = false, limited = false, sequence = 1, searchId = null, bytes = 0;
    try {
      const response = await this.fetcher("/sessions/" + encodeURIComponent(request.session) + "/search", {
        method:"POST", headers:{...this.headers(), "Content-Type":"application/json"},
        body:JSON.stringify({query, case_sensitive:false, limit:MAX_RESULTS}),
        signal:request.controller.signal, cache:"no-store"
      });
      if (!request.current()) { await response.body?.cancel(); return; }
      checkResponse(response);
      if (!response.body) throw new Error("Search response is missing");
      await readEvents(response.body, ({event, data}) => {
        if (!request.current()) return false;
        if (event === "search-error") throw new Error(data);
        if (event !== "search-page") throw new Error("Unexpected search event");
        const page = JSON.parse(data);
        if (page.session_id !== request.session || typeof page.search_id !== "string" || !page.search_id
            || (searchId !== null && searchId !== page.search_id) || page.sequence !== sequence++
            || typeof page.complete !== "boolean" || !Array.isArray(page.matches) || page.matches.length > 64
            || page.error) throw new Error("Invalid search page identity, order or bounds");
        searchId = page.search_id;
        for (const found of page.matches) {
          if (!Number.isSafeInteger(found.line) || found.line < 0 || !Number.isSafeInteger(found.column)
              || found.column < 0 || typeof found.preview !== "string") throw new Error("Invalid search result");
          const size = encoder.encode(found.preview).length;
          if (this.matches.length === MAX_RESULTS || bytes + size > MAX_PREVIEW_BYTES) { limited = true; break; }
          bytes += size; this.matches.push(found);
        }
        complete = page.complete;
        limited ||= this.matches.length === MAX_RESULTS;
        this.status = limited ? `${this.matches.length} results · display limit reached; narrow the query`
          : complete ? `Search complete: ${this.matches.length} results` : `Searching: ${this.matches.length} results so far`;
        this.running = !complete && !limited;
        this.onChange(this);
        return !complete && !limited;
      }, request.controller.signal);
      if (!request.current()) return;
      if (!complete && !limited) throw new Error("Search disconnected before completion; run it again");
    } catch (error) { this.failed(error, request); }
    finally {
      request.finish(); request.controller.abort();
      if (request.current()) { this.running = false; this.controller = null; this.onChange(this); }
    }
  }
  async reveal(found) {
    if (!this.session || !this.matches.includes(found)) return;
    this.cancel(false);
    this.onInspect();
    const request = this.begin("Loading read-only history…");
    try {
      const response = await this.fetcher("/sessions/" + encodeURIComponent(request.session) + "/history-line/" + found.line, {
        headers:this.headers(), signal:request.controller.signal, cache:"no-store"
      });
      if (!request.current()) { await response.body?.cancel(); return; }
      checkResponse(response);
      const frame = await boundedJson(response);
      if (!request.current()) return;
      if (typeof frame.text !== "string" || !frame.text.split("\n").some(line => line.trimEnd() === found.preview.trimEnd()))
        throw new Error("History changed; search again to locate this result");
      this.status = "Read-only search history · row positions may change";
      this.onFrame(frame, found);
    } catch (error) { this.failed(error, request); }
    finally {
      request.finish(); request.controller.abort();
      if (request.current()) { this.running = false; this.controller = null; this.onChange(this); }
    }
  }
}
