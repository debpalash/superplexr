import test from "node:test";
import assert from "node:assert/strict";
import {BrowserSearch, MAX_PREVIEW_BYTES} from "./search.mjs";

const hit = (preview = "needle 界", line = 1) => ({line, column:0, preview});
const page = (sequence, complete, matches = [hit()], extra = {}) => ({
  session_id:"session-a", search_id:"query-a", sequence, complete, matches, error:null, ...extra
});
const event = data => `event: search-page\ndata: ${JSON.stringify(data)}\n\n`;
function response(pages, split = false) {
  const bytes = new TextEncoder().encode(pages.map(event).join(""));
  return new Response(new ReadableStream({start(controller) {
    if (split) for (const byte of bytes) controller.enqueue(Uint8Array.of(byte));
    else controller.enqueue(bytes);
    controller.close();
  }}));
}
function setup(fetcher, callbacks = {}) {
  const search = new BrowserSearch({headers:() => ({Authorization:"Bearer test"}), fetcher, ...callbacks});
  search.attach("session-a");
  return search;
}
function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return {promise, resolve};
}

test("incremental Unicode pages use a scoped POST body and only explicit completion succeeds", async () => {
  const counts = [];
  const search = setup(async (url, options) => {
    assert.equal(url, "/sessions/session-a/search");
    assert.equal(options.method, "POST");
    assert.equal(options.headers.Authorization, "Bearer test");
    assert.equal(JSON.parse(options.body).query, "needle 界");
    return response([page(1, false), page(2, true, [hit("other", 2)])], true);
  }, {onChange:state => counts.push(state.matches.length)});
  await search.start("needle 界");
  assert.equal(search.matches.length, 2);
  assert.equal(search.matches[0].preview, "needle 界");
  assert.ok(counts.includes(1));
  assert.match(search.status, /Search complete: 2/);
  assert.equal(search.running, false);
});

test("truncated or invalid streams clear partial hits and never retry", async () => {
  for (const pages of [
    [page(1, false)],
    [page(1, false), page(3, true)],
    [page(1, false), page(2, true, [], {session_id:"other"})],
    [page(1, false), page(2, true, [], {search_id:"other"})],
    [page(1, false), page(2, true, [], {error:"revoked"})],
  ]) {
    let calls = 0;
    const search = setup(async () => { calls++; return response(pages); });
    await search.start("needle");
    assert.equal(calls, 1);
    assert.deepEqual(search.matches, []);
    assert.match(search.status, /disconnected|Invalid/);
  }
});

test("replaced requests cannot publish late results even if fetch ignores abort", async () => {
  const old = deferred(); let oldSignal;
  const search = setup(async (_, options) => {
    if (JSON.parse(options.body).query === "old") { oldSignal = options.signal; return old.promise; }
    return response([page(1, true, [hit("new")])]);
  });
  const pending = search.start("old");
  await search.start("new");
  assert.equal(oldSignal.aborted, true);
  old.resolve(response([page(1, true, [hit("old")])]));
  await pending;
  assert.equal(search.matches[0].preview, "new");
});

test("cancel interrupts active reads while keeping explicitly partial results", async () => {
  const partial = deferred(); let activeSignal;
  const search = setup(async (_, {signal}) => { activeSignal = signal; return new Response(new ReadableStream({
    start(controller) {
      signal.addEventListener("abort", () => controller.error(signal.reason), {once:true});
      controller.enqueue(new TextEncoder().encode(event(page(1, false))));
    }
  })); }, {onChange:state => { if (state.matches.length) partial.resolve(); }});
  const pending = search.start("needle");
  await partial.promise;
  search.cancel(false, "Partial results retained");
  await pending;
  assert.equal(search.matches.length, 1);
  assert.equal(search.status, "Partial results retained");
  assert.equal(search.running, false);
  assert.equal(activeSignal.aborted, true);
});

test("query and retained preview limits are byte bounds, with explicit limited state", async () => {
  let calls = 0;
  const search = setup(async () => {
    calls++;
    const preview = "界".repeat(20000);
    return response(Array.from({length:40}, (_, i) => page(i + 1, false, [hit(preview, i)])));
  });
  await search.start("界".repeat(342));
  assert.equal(calls, 0);
  await search.start("needle");
  assert.equal(calls, 1);
  const bytes = search.matches.reduce((sum, found) => sum + new TextEncoder().encode(found.preview).length, 0);
  assert.ok(bytes <= MAX_PREVIEW_BYTES);
  assert.ok(bytes > MAX_PREVIEW_BYTES - 60000);
  assert.match(search.status, /display limit reached/);
});

test("opening history is read-only, checks the original preview, and rejects stale rows", async () => {
  const ordering = [], frames = [];
  let text = "needle 界\nother";
  const search = setup(async url => {
    if (url.endsWith("/search")) return response([page(1, true)]);
    ordering.push("fetch");
    assert.equal(url, "/sessions/session-a/history-line/1");
    return Response.json({text});
  }, {onInspect:() => ordering.push("inspect"), onFrame:frame => frames.push(frame)});
  await search.start("needle");
  await search.reveal(search.matches[0]);
  assert.deepEqual(ordering, ["inspect", "fetch"]);
  assert.equal(frames.length, 1);
  text = "replacement";
  await search.reveal(search.matches[0]);
  assert.equal(frames.length, 1);
  assert.deepEqual(search.matches, []);
  assert.match(search.status, /History changed/);
});

test("session replacement discards a late history frame", async () => {
  const old = deferred(), frames = [];
  const search = setup(async url => url.endsWith("/search") ? response([page(1, true)]) : old.promise,
    {onFrame:frame => frames.push(frame)});
  await search.start("needle");
  const pending = search.reveal(search.matches[0]);
  search.attach("session-b");
  old.resolve(Response.json({text:"needle 界"}));
  await pending;
  assert.deepEqual(frames, []);
  assert.deepEqual(search.matches, []);
});

test("authority rejection ends access while capacity rejection remains retryable by user", async () => {
  for (const status of [401, 403, 404, 429]) {
    let ended = 0;
    const search = setup(async () => new Response(null, {status}), {onEnded:() => ended++});
    await search.start("needle");
    assert.equal(ended, status === 429 ? 0 : 1);
    assert.match(search.status, status === 429 ? /busy/ : /access ended/);
    assert.deepEqual(search.matches, []);
  }
});
