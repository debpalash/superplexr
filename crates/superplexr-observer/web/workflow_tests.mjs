import test from "node:test";
import assert from "node:assert/strict";
import {readFile} from "node:fs/promises";
import {WorkflowInspector} from "./workflow.mjs";

// A small DOM adapter keeps the tests dependency-free while driving the actual
// inspector event handlers, rendering and streamed HTTP response reader.
class Element extends EventTarget {
  constructor(tag = "div") { super(); this.tag = tag; this.children = []; this.value = ""; this.hidden = false; this.disabled = false; this.open = true; this.text = ""; }
  set textContent(value) { this.text = String(value); this.children = []; }
  get textContent() { return this.text + this.children.map(child => child.textContent).join(""); }
  append(...children) { this.children.push(...children); }
  replaceChildren(...children) { this.text = ""; this.children = children; }
  querySelector(selector) { return this.selectors?.[selector] ?? this.querySelectorAll(selector)[0] ?? null; }
  querySelectorAll(selector) { return this.children.flatMap(child => [...(child.tag === selector ? [child] : []), ...child.querySelectorAll(selector)]); }
  click() { if (!this.disabled) this.dispatchEvent(new Event("click")); }
  set innerHTML(_) { assert.fail("Workflow data must never become HTML"); }
}
globalThis.document = {createElement:tag => new Element(tag)};
const id = n => `00000000-0000-0000-0000-${n.toString(16).padStart(12,"0")}`;
const mission = id(500), verifier = id(1), subject = id(600);
const entry = n => ({verifier_run_id:id(n), subject_run_id:subject, phase:"finished", outcome:"Succeeded", subject_disposition:"awaiting_review", primary_session_id:null});
const page = (entries = [entry(1)], extra = {}) => ({mission_id:mission, mission_version:7, after:null, limit:32, entries, next_after:null, ...extra});
const status = (extra = {}) => ({mission_id:mission, verifier_run_id:verifier, subject_run_id:subject, phase:"finished", outcome:"Succeeded", subject_disposition:"awaiting_review", candidate_revision:"<script>literal text</script>", candidate_sha256:"a".repeat(64), receipt_count:1, passing_receipt_count:0, receipts:[{artifact_id:id(900),verdict:"failed"}], receipts_truncated:false, observation_only:true, evidence_rechecked:false, ...extra});
const response = data => Response.json(data);
const settle = () => new Promise(resolve => setTimeout(resolve,0));
function deferred() { let resolve; const promise = new Promise(done => { resolve = done; }); return {promise, resolve}; }
function harness(fetcher = async () => response(page())) {
  const root = new Element("details");
  root.selectors = Object.fromEntries(["form","[name=mission]","[name=verifier]","[role=status]",".workflow-result",".workflow-discover","button[type=submit]",".workflow-page",".workflow-page-info",".workflow-verifiers",".workflow-next"].map(selector => [selector, new Element(selector.includes("button") || selector.includes("discover") || selector.includes("next") ? "button" : "div")]));
  const calls = [];
  const inspector = new WorkflowInspector(root, {headers:() => ({Authorization:"Bearer test"}), fetcher:async function(url,options) {
    assert.equal(this, undefined, "fetch must not be called with the inspector as receiver");
    calls.push({url,options}); return fetcher(url,options);
  }});
  inspector.setEnabled(true); root.open = true; inspector.mission.value = mission; inspector.verifier.value = verifier;
  return {inspector, root, calls};
}

test("markup keeps Mission required and discovery independent of a verifier ID", async () => {
  const html = await readFile(new URL("./index.html",import.meta.url),"utf8");
  assert.match(html, /name="mission"[^>]*required/);
  assert.doesNotMatch(html.match(/<input name="verifier"[^>]*>/)[0], /required/);
  assert.match(html, /type="button" class="workflow-discover"/);
  assert.match(html, /type="submit">Inspect status/);
});

test("discovery is explicit, requires Mission ID, and sends only an authenticated GET", async () => {
  const {inspector,root,calls} = harness();
  assert.equal(calls.length,0); root.dispatchEvent(new Event("toggle")); assert.equal(calls.length,0);
  inspector.mission.value = "invalid"; await inspector.discover(); assert.equal(calls.length,0);
  inspector.mission.value = mission; inspector.verifier.value = "";
  inspector.discoverButton.click(); await settle();
  assert.equal(calls.length,1); assert.equal(calls[0].url,`/missions/${mission}/verifiers?limit=32`);
  assert.equal(calls[0].options.method,"GET"); assert.equal(calls[0].options.cache,"no-store");
  assert.equal(calls[0].options.headers.Authorization,"Bearer test"); assert.equal(calls[0].options.body,undefined);
  assert.equal(inspector.entries.children.length,1); assert.equal(inspector.nextButton.disabled,true);
  assert.match(inspector.pageInfo.textContent,/Mission version 7/);
  assert.match(inspector.message.textContent,/no automatic refresh/);
});

test("next-page reads replace the bounded list and selecting a verifier reads compact status", async () => {
  const first = Array.from({length:32},(_,n) => entry(n+1));
  const {inspector,calls} = harness(async url => response(url.endsWith("/status") ? status({verifier_run_id:id(33)})
    : url.includes("after=") ? page([entry(33)],{after:id(32),mission_version:8}) : page(first,{next_after:id(32)})));
  await inspector.discover(); assert.equal(inspector.entries.children.length,32); assert.equal(inspector.nextButton.disabled,false);
  inspector.nextButton.click(); await settle();
  assert.equal(calls.length,2); assert.match(calls[1].url,/&after=00000000-0000-0000-0000-000000000020$/);
  assert.equal(inspector.entries.children.length,1); assert.equal(inspector.nextButton.disabled,true);
  inspector.entries.querySelector("button").click(); await settle();
  assert.equal(calls.length,3); assert.equal(calls[2].url,`/missions/${mission}/verifiers/${id(33)}/status`);
  assert.equal(inspector.verifier.value,id(33)); assert.equal(inspector.entries.children.length,1);
  assert.match(inspector.result.textContent,/finished · Succeeded/);
  assert.match(inspector.result.textContent,/0 match the domain passing predicate/);
  assert.match(inspector.result.textContent,/awaiting_review/);
  assert.match(inspector.result.textContent,/<script>literal text<\/script>/);
  assert.match(inspector.result.textContent,/evidence files were not rechecked/);
});

test("direct ID form submission works without a list and rejects missing verifier IDs", async () => {
  const {inspector,calls} = harness(async () => response(status()));
  inspector.verifier.value = ""; await inspector.inspect(); assert.equal(calls.length,0);
  inspector.verifier.value = verifier;
  const event = new Event("submit",{cancelable:true}); inspector.form.dispatchEvent(event); await settle();
  assert.equal(event.defaultPrevented,true); assert.equal(calls.length,1); assert.equal(inspector.result.children.length,3);
});

test("malformed, oversized and inconsistent verifier pages render nothing and do not retry", async () => {
  const bad = [null, page([],{mission_id:id(999)}), page([],{mission_version:-1}), page([],{mission_version:1.1}),
    page([],{after:id(1)}), page([],{limit:64}), page(Array.from({length:33},(_,n) => entry(n+1))),
    page([null]), page([entry(2),entry(1)]), page([entry(1),entry(1)]), page([{...entry(1),primary_session_id:"bad"}]),
    page([{...entry(1),subject_disposition:"unknown"}]), page([{...entry(1),outcome:"passed"}]),
    page([],{next_after:id(1)}), page([entry(1)],{next_after:id(1)}), page([],{padding:"界".repeat(22000)})];
  for (const data of bad) {
    const {inspector,calls} = harness(async () => response(data)); await inspector.discover();
    assert.equal(calls.length,1); assert.equal(inspector.entries.children.length,0); assert.equal(inspector.page.hidden,true);
    assert.match(inspector.message.textContent,/Invalid|exceeded/); assert.equal(inspector.discoverButton.disabled,false);
  }
});

test("bad receipt summaries and status flags never publish partial status", async () => {
  for (const data of [null,status({receipts:[null]}),status({receipts:[{artifact_id:id(1),verdict:"wrong"}]}),
    status({passing_receipt_count:2}),status({receipts_truncated:"yes"}),status({evidence_rechecked:true}),
    status({observation_only:false}),status({candidate_revision:"x".repeat(257)}),status({receipts:Array(17).fill({})})]) {
    const {inspector} = harness(async () => response(data)); await inspector.inspect();
    assert.equal(inspector.result.children.length,0); assert.match(inspector.message.textContent,/Invalid/);
  }
});

test("Mission edits, panel close and disabling invalidate late responses even when fetch ignores abort", async () => {
  for (const cancel of [i => { i.mission.value=id(501); i.form.dispatchEvent(new Event("input")); },
    i => { i.root.open=false; i.root.dispatchEvent(new Event("toggle")); }, i => i.setEnabled(false)]) {
    const old = deferred(); const {inspector,calls} = harness(() => old.promise);
    const pending = inspector.discover(); cancel(inspector);
    assert.equal(calls[0].options.signal.aborted,true);
    old.resolve(response(page())); await pending;
    assert.equal(inspector.entries.children.length,0); assert.equal(inspector.result.children.length,0);
  }
});

test("a superseded list cannot overwrite newer direct status or re-enable pending controls", async () => {
  const old = deferred(), current = deferred();
  const {inspector,calls} = harness(url => url.endsWith("/status") ? current.promise : old.promise);
  const first = inspector.discover(), second = inspector.inspect();
  old.resolve(response(page())); await first;
  assert.equal(inspector.inspectButton.disabled,true); assert.equal(inspector.entries.children.length,0);
  current.resolve(response(status())); await second;
  assert.equal(calls.length,2); assert.equal(inspector.inspectButton.disabled,false); assert.equal(inspector.result.children.length,3);
});

test("stream cancellation and 64 KiB overflow release unread response data", async () => {
  for (const oversized of [false,true]) {
    const started = deferred(); let cancelled=false;
    const {inspector} = harness(async () => new Response(new ReadableStream({start(controller) {
      controller.enqueue(new TextEncoder().encode(oversized ? "x".repeat(65537) : "{")); started.resolve();
    }, cancel() { cancelled=true; }})));
    const pending = inspector.discover(); await started.promise; await settle();
    if (!oversized) inspector.clear();
    await pending; assert.equal(cancelled,true); assert.equal(inspector.entries.children.length,0);
    if (oversized) assert.match(inspector.message.textContent,/exceeded/);
  }
});

test("HTTP denials, non-JSON errors and empty responses stay bounded and release their bodies", async () => {
  for (const code of [401,403,500,204]) {
    let cancelled=false;
    const {inspector,calls} = harness(async () => code === 204 ? new Response(null,{status:204})
      : new Response(new ReadableStream({start(controller) { controller.enqueue(new TextEncoder().encode("no")); },cancel() { cancelled=true; }}),{status:code}));
    await inspector.discover(); assert.equal(calls.length,1); assert.equal(inspector.entries.children.length,0);
    assert.match(inspector.message.textContent,/unavailable|failed|empty/);
    if (code !== 204) assert.equal(cancelled,true);
  }
});

test("timeouts cancel a stalled body and never publish a late fetch response", async t => {
  t.mock.timers.enable({apis:["setTimeout"]});
  const late = deferred(); const {inspector} = harness(() => late.promise);
  const pending = inspector.discover(); t.mock.timers.tick(10000);
  late.resolve(response(page())); await pending;
  assert.equal(inspector.entries.children.length,0); assert.match(inspector.message.textContent,/timed out/);
  assert.equal(inspector.discoverButton.disabled,false);

  const started = deferred(); let cancelled=false;
  const {inspector:streaming} = harness(async () => new Response(new ReadableStream({start(controller) {
    controller.enqueue(new TextEncoder().encode("{")); started.resolve();
  },cancel() { cancelled=true; }})));
  const stalled = streaming.discover(); await started.promise;
  // Let the response continuation attach its reader before advancing time.
  await Promise.resolve(); await Promise.resolve(); t.mock.timers.tick(10000); await stalled;
  assert.equal(cancelled,true); assert.match(streaming.message.textContent,/timed out/);
  assert.equal(streaming.entries.children.length,0); assert.equal(streaming.discoverButton.disabled,false);
});

test("untrusted error text remains bounded and a failed continuation clears the old page", async () => {
  const first = Array.from({length:32},(_,n) => entry(n+1));
  const {inspector,calls} = harness(async url => url.includes("after=")
    ? Response.json({error:"x".repeat(4096)},{status:400}) : response(page(first,{next_after:id(32)})));
  await inspector.discover(); await inspector.discover(inspector.nextAfter);
  assert.equal(calls.length,2); assert.equal(inspector.message.textContent.length,512);
  assert.equal(inspector.entries.children.length,0); assert.equal(inspector.nextAfter,null);
});

test("disable clears scoped data and IDs, and enabling alone does not fetch", async () => {
  const {inspector,calls} = harness(); await inspector.discover(); inspector.setEnabled(false);
  assert.equal(inspector.mission.value,""); assert.equal(inspector.verifier.value,""); assert.equal(inspector.root.hidden,true);
  assert.equal(inspector.entries.children.length,0); assert.equal(inspector.nextAfter,null);
  await inspector.discover(); await inspector.inspect(); inspector.setEnabled(true); assert.equal(calls.length,1);
});
