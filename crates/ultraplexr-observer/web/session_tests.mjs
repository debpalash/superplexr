import test from "node:test";
import assert from "node:assert/strict";
import {readFile} from "node:fs/promises";
import vm from "node:vm";
import {domFixture} from "./dom-fixture.mjs";
import {AccessEnded,followFeed} from "./stream.mjs";
import {FrameContinuityError,FrameDecoder} from "./frames.mjs";

// Resolve the browser's root-relative module URL for Node without changing the
// production module or replacing its actual SSE decoder/reconnect behavior.
const source = await readFile(new URL("./sessions.mjs",import.meta.url),"utf8");
const {followSessions,sessionLabel,sessionDetails} = await import(`data:text/javascript;base64,${Buffer.from(source.replace('"/stream.mjs"',JSON.stringify(new URL("./stream.mjs",import.meta.url).href))).toString("base64")}`);
const id = n => `00000000-0000-0000-0000-${n.toString(16).padStart(12,"0")}`;
const entry = (n,extra={}) => ({session_id:id(n),archived:false,status:"running",executable:"/bin/sh",title:null,directory:"/project",...extra});
const event = (event,data) => ({event,data:typeof data === "string" ? data : JSON.stringify(data)});
const marker = (type,generation=id(100)) => event("collection",{type,generation});
const sse = events => new Response([...events,event("complete","test complete")].map(({event,data}) => `event: ${event}\ndata: ${data}\n\n`).join(""));
async function run(t, events, options={}) {
  const publications=[],features=[],statuses=[]; let calls=0;
  t.mock.method(globalThis,"fetch",async (url,request) => {
    assert.equal(url,"/sessions/events"); assert.equal(request.headers.Authorization,"Bearer fixture");
    assert.equal(request.body,undefined); calls++; return sse(events);
  });
  await followSessions({headers:{Authorization:"Bearer fixture"},signal:new AbortController().signal,onMode:()=>{},
    onSessions:entries => publications.push(entries),onFeatures:value => features.push(value),onStatus:value => statuses.push(value),...options});
  return {publications,features,statuses,calls};
}

test("a complete catalog replaces missing sessions atomically; partial snapshots never remove them", async t => {
  const {publications,calls} = await run(t,[event("configuration","observer"),marker("snapshot_begin"),event("session",entry(2)),event("session",entry(1)),marker("snapshot_end"),
    marker("snapshot_begin",id(101)),event("session",entry(2,{title:"renamed"})),marker("snapshot_end",id(999)),
    marker("snapshot_end",id(101)),marker("snapshot_begin",id(102)),event("session",entry(3))]);
  assert.equal(calls,1); assert.deepEqual(publications.map(rows => rows.map(row => row.session_id)),[[id(1),id(2)],[id(2)]]);
  assert.equal(publications[0][1].title,null); assert.equal(publications[1][0].title,"renamed");
});

test("live title changes preserve catalog membership, duplicate entries are quiet, archives remove exactly one session", async t => {
  const {publications} = await run(t,[event("session",entry(99)),event("configuration","observer"),marker("snapshot_begin"),event("session",entry(1)),event("session",entry(2)),marker("snapshot_end"),
    event("session",entry(1)),event("session",entry(1,{title:"new title"})),event("session",entry(2,{archived:true}))]);
  assert.equal(publications.length,3);
  assert.deepEqual(publications.map(rows => rows.map(row => row.session_id)),[[id(1),id(2)],[id(1),id(2)],[id(1)]]);
  assert.equal(publications[1][0].title,"new title");
});

test("replacement snapshot generations discard partial predecessors and empty snapshots publish removal", async t => {
  const {publications,features} = await run(t,[event("configuration","observer"),event("features",{workflow_read:true}),
    marker("snapshot_begin"),event("session",entry(1)),marker("snapshot_begin",id(101)),event("session",entry(2)),
    marker("snapshot_end"),marker("snapshot_end",id(101)),event("configuration","observer"),marker("snapshot_begin",id(102)),marker("snapshot_end",id(102))]);
  assert.deepEqual(publications.map(rows => rows.map(row => row.session_id)),[[id(2)],[]]);
  assert.deepEqual(features,[{workflow_read:false},{workflow_read:true},{workflow_read:false}]);
});

test("malformed or excessive catalogs fail closed instead of publishing partial membership", async t => {
  for (const update of [event("session",entry(1,{title:"x".repeat(1025)})),event("session",entry(1,{directory:42})),
    event("collection",{type:"unknown",generation:id(100)}),event("features",{workflow_read:"yes"})]) {
    await assert.rejects(run(t,[event("configuration","observer"),marker("snapshot_begin"),update],{onSessions:() => assert.fail("invalid partial snapshot must not publish")}),/Invalid session-list/);
  }
  await assert.rejects(run(t,[event("configuration","observer"),marker("snapshot_begin"),...Array.from({length:4097},(_,n) => event("session",entry(n+1)))],{onSessions:() => assert.fail("oversized partial snapshot must not publish")}),/4,096 entries/);
});

test("labels strip control and direction overrides while preserving stable identity fallbacks", () => {
  assert.equal(sessionLabel(entry(1,{title:"\u202e\x1b new\n name \u2069"})),"new name");
  assert.equal(sessionLabel(entry(1,{directory:"C:\\projects\\demo\\"})),"demo");
  assert.equal(sessionLabel(entry(1,{directory:null,executable:"tool"})),"tool");
  assert.equal(sessionLabel(entry(1,{directory:null,executable:null})),id(1).slice(0,8));
  assert.equal(sessionDetails(entry(1,{executable:"sh\x00",status:"running"})),"sh · running");
});

test("the actual navigation renderer keeps focus on title updates and retires removed selections", async () => {
  const observer = await readFile(new URL("./observer.js",import.meta.url),"utf8");
  const start = observer.indexOf("function renderSessions("), end = observer.indexOf("function renderFrame(",start);
  assert.ok(start >= 0 && end > start,"navigation renderer seam exists");
  const {document} = domFixture(); const elements = new Map();
  const $ = id => { if (!elements.has(id)) elements.set(id,document.createElement(id === "sessions" ? "nav" : "div")); return elements.get(id); };
  let aborted=0,cleared=0; const reconciled=[];
  const context = vm.createContext({document,$,sessionLabel,sessionDetails,selected:id(1),catalogReady:false,
    sideViews:{reconcile:rows => reconciled.push(rows.map(row => row.session_id))},stream:{abort:() => aborted++},clearFeed:() => cleared++,updateSideViewButton:()=>{}});
  vm.runInContext(observer.slice(start,end),context);
  context.renderSessions([entry(1),entry(2)]);
  const first = $("sessions").firstElementChild; first.focus();
  context.renderSessions([entry(1,{title:"changed display title"}),entry(2)]);
  assert.equal($("sessions").firstElementChild,first); assert.equal(document.activeElement,first);
  assert.equal(first.firstElementChild.textContent,"changed display title");
  assert.equal(first.dataset.id,id(1)); assert.equal(first.getAttribute("aria-current"),"true");
  assert.equal(context.selected,id(1)); assert.equal(aborted,0); assert.equal(cleared,0);
  context.renderSessions([entry(2)]);
  assert.equal(context.selected,null); assert.equal(aborted,1); assert.equal(cleared,1);
  assert.deepEqual($("sessions").children.map(button => button.dataset.id),[id(2)]);
  assert.equal($("title").textContent,"Session no longer available");
  assert.equal($("copy").disabled,true); assert.deepEqual(reconciled.at(-1),[id(2)]);
});

test("main attachment continuity failures reconnect once without deltas and never reacquire input", async () => {
  const observer = await readFile(new URL("./observer.js",import.meta.url),"utf8");
  const start = observer.indexOf("async function selectSession("), end = observer.indexOf("async function connect(",start);
  assert.ok(start >= 0 && end > start,"attachment lifecycle seam exists");
  for (const malformedFallback of [false,true]) {
    const {document} = domFixture(), nodes = new Map(), calls=[],rendered=[]; let detached=0,cleared=0;
    const $ = id => { if (!nodes.has(id)) nodes.set(id,document.createElement("div")); return nodes.get(id); };
    const full = {revision:"1",sequence:1,rows:1,text:"original",title:null,directory:null,status:"running"};
    const replies = [sse([event("frame",full),event("frame-delta",{base_revision:"wrong"})]),
      sse([event("frame",malformedFallback ? {text:42} : {...full,revision:"2",sequence:2,text:"fresh full frame"})])];
    const context = vm.createContext({document,$,AbortController,FrameDecoder,FrameContinuityError,AccessEnded,
      headers:() => ({Authorization:"Bearer fixture"}),stream:null,selected:null,paused:false,historyOffset:0,latestFrame:null,
      updateSideViewButton:()=>{},clearFeed:() => cleared++,terminalDisplay:{clear:()=>{}},search:{attach:()=>{},cancel:()=>{}},
      control:{detach:() => detached++,attach:() => assert.fail("read-only frames must not attach input")},renderFrame:value => rendered.push(value),
      followFeed:options => followFeed({...options,sleep:async()=>{},fetcher:async(url,options) => {
        calls.push({url,headers:{...options.headers},body:options.body});
        assert.ok(replies.length,"no unbounded reconnect"); return replies.shift();
      }})});
    vm.runInContext(observer.slice(start,end),context);
    await context.selectSession(id(1));
    assert.equal(calls.length,2); assert.equal(calls[0].headers["X-Ultraplexr-Frames"],"row-delta-v1");
    assert.equal(calls[1].headers["X-Ultraplexr-Frames"],undefined);
    assert.ok(calls.every(call => call.url === `/sessions/${id(1)}/events` && call.headers.Authorization === "Bearer fixture" && call.body === undefined));
    assert.ok(detached >= 1);
    if (malformedFallback) {
      assert.equal(rendered.length,1); assert.match($("feed-status").textContent,/Invalid terminal frames; reconnect explicitly/); assert.equal(cleared,2);
    } else {
      assert.deepEqual(rendered.map(value => value.text),["original","fresh full frame"]); assert.equal(cleared,1);
    }
  }
});
