import test from "node:test";
import assert from "node:assert/strict";
import {AccessEnded, followFeed, readEvents, retryDelay} from "./stream.mjs";

const response = (text, status=200) => new Response(text, {status});
test("SSE handles every-byte UTF8 and CRLF boundaries", async () => {
  const bytes = new TextEncoder().encode('event: frame\r\ndata: {"text":"héllo 🌍"}\r\n\r\n');
  const body = new ReadableStream({start(c){ for (const byte of bytes) c.enqueue(Uint8Array.of(byte)); c.close(); }});
  const events = [];
  await readEvents(body, e => events.push(e), new AbortController().signal);
  assert.deepEqual(events, [{event:"frame", data:'{"text":"héllo 🌍"}'}]);
});
test("SSE bounds multiline packets even when each network chunk is small", async () => {
  const line = new TextEncoder().encode("data: " + "x".repeat(1024) + "\n");
  const body = new ReadableStream({pull(controller){ controller.enqueue(line); }});
  await assert.rejects(readEvents(body, ()=>assert.fail("oversized packet must not dispatch"), new AbortController().signal), /Oversized event/);
});
test("transient reconnect preserves URL and authorization without sending input", async () => {
  const calls = [], retries = [], events = [];
  const replies = [response("",503), response('event: frame\ndata: {"sequence":1}\n\n'), response('event: frame\ndata: {"sequence":3}\n\nevent: complete\ndata: done\n\n')];
  await followFeed({url:"/sessions/stable-id/events", headers:{Authorization:"Bearer observer"}, signal:new AbortController().signal,
    onEvent:e=>events.push(e), onRetry:(_,ms)=>retries.push(ms), sleep:async()=>{},
    fetcher:async(url,options)=>{ calls.push({url,options}); return replies.shift(); }});
  assert.equal(calls.length,3);
  assert.ok(calls.every(c=>c.url==="/sessions/stable-id/events" && c.options.headers.Authorization==="Bearer observer" && !c.options.body));
  assert.deepEqual(events.map(e=>e.event), ["frame","frame","complete"]);
  assert.deepEqual(retries,[250,250]);
});
for (const status of [401,403,404]) test("does not retry denied scope " + status, async () => {
  let calls=0;
  await assert.rejects(followFeed({url:"/sessions/id/events", headers:{}, signal:new AbortController().signal,
    onEvent:()=>{}, onRetry:()=>assert.fail("must not retry"), fetcher:async()=>{ calls++; return response("",status); }}), AccessEnded);
  assert.equal(calls,1);
});
test("revocation event ends an established feed", async () => {
  await assert.rejects(followFeed({url:"/sessions/id/events", headers:{}, signal:new AbortController().signal,
    onEvent:()=>{}, onRetry:()=>assert.fail("must not retry"), fetcher:async()=>response("event: ended\ndata: revoked\n\n")}), AccessEnded);
});
test("oversized record stops both initial and established feed retries with a resource message", async () => {
  for (const reply of [response("",413), response("event: ended\ndata: Subscription exceeds the client receive limit\n\n")]) {
    let calls=0;
    await assert.rejects(followFeed({url:"/sessions/id/events", headers:{}, signal:new AbortController().signal,
      onEvent:()=>{}, onRetry:()=>assert.fail("permanent limit must not retry"), fetcher:async()=>{ calls++; return reply; }}), /receive limit/);
    assert.equal(calls,1);
  }
});
test("backoff is capped and detach cancels retries", async () => {
  assert.deepEqual([0,1,2,3,4,20].map(retryDelay),[250,500,1000,2000,4000,4000]);
  const controller=new AbortController(); let calls=0;
  await followFeed({url:"/sessions/id/events", headers:{}, signal:controller.signal, onEvent:()=>{},
    onRetry:()=>controller.abort(), sleep:async()=>{}, fetcher:async()=>{ calls++; throw new Error("offline"); }});
  assert.equal(calls,1);
});
