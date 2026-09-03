// Streams, driven the way a browser would: a viewer link minted on the host,
// a viewer admitted by it and held to it, presence the owner can see,
// revocation ending the viewer, and a 50-viewer fan-out measured in bytes.
import { execSync } from "node:child_process";
process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";
const [,, base, socket, bin] = process.argv;
const modOf = (file) => import(`file://${process.cwd()}/web/${file}`);
const { Kind, FrameReader, Sequencer, encodeJson, hello, json, uuid, PROTOCOL_VERSION } = await modOf("wire.js");
const { decodeFullFrame, decodeFrameDelta, applyDelta } = await modOf("frames.js");

const step = (s) => console.log(`▸ ${s}`);
const fail = (s) => { console.log(`   FAIL: ${s}`); process.exit(1); };
process.on("unhandledRejection", (e) => fail(`${e.code ?? ""} ${e.message ?? JSON.stringify(e)}`));
const host = (args) => JSON.parse(execSync(`${bin} --socket ${socket} ${args}`).toString());
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function connect({ shareToken = null, deviceId = uuid(), token = null } = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${base.replace(/^http/, "ws")}/ws`);
    ws.binaryType = "arraybuffer";
    const reader = new FrameReader();
    const sequencer = new Sequencer();
    const waiters = new Map();
    const streams = new Map();
    const clientId = uuid();
    const stats = { bytes: 0, frames: 0 };
    let welcome = null;
    ws.onopen = () => {
      const greeting = hello(clientId, deviceId, { deviceToken: token });
      greeting.share_token = shareToken;
      ws.send(encodeJson(Kind.Hello, 0, greeting, sequencer));
    };
    ws.onmessage = (ev) => {
      stats.bytes += ev.data.byteLength;
      reader.push(new Uint8Array(ev.data));
      for (const frame of reader.take()) {
        if (!welcome) {
          if (frame.kind === Kind.Welcome) { welcome = json(frame); resolve(api); }
          else if (frame.kind === Kind.Close) reject(json(frame));
          continue;
        }
        if (frame.streamId === 0 && frame.kind === Kind.Response) {
          const r = json(frame); const w = waiters.get(r.request_id); waiters.delete(r.request_id);
          if (!w) continue;
          if (r.result.status === "success") w.resolve(r.result.body); else w.reject(r.result);
        } else if (frame.streamId !== 0) {
          stats.frames++;
          streams.get(frame.streamId)?.(frame);
        }
      }
    };
    ws.onclose = (ev) => { api.closed = true; reject({ code: "closed", message: ev.reason }); };
    function request(action, extra = {}) {
      const id = uuid();
      ws.send(encodeJson(Kind.Request, 0, { version: PROTOCOL_VERSION, client_id: clientId, request_id: id, surface_id: null, control_epoch: null, share_token: shareToken, ...extra, action }, sequencer));
      return new Promise((res, rej) => waiters.set(id, { resolve: res, reject: rej }));
    }
    const api = { ws, request, streams, stats, closed: false, get welcome() { return welcome; } };
  });
}

step("1. a session, and a viewer link for it minted on the host");
const sid = host(`terminal-new --cwd /tmp --program /bin/sh -- -c 'printf "STREAM-MARKER\\n"; i=0; while [ $i -lt 600 ]; do echo "line $i of a stream that scrolls at twenty lines a second"; i=$((i+1)); sleep 0.05; done'`).terminal.session_id;
const link = host(`stream ${sid} --label smoke-viewers`);
if (!link.url?.includes("#share=")) fail(`no url in ${JSON.stringify(link)}`);
const shareToken = link.token;
console.log(`   ${link.url.replace(shareToken, "…")} · role ${link.role}`);

step("2. a viewer is admitted by the link, sees the session, and only that");
const viewer = await connect({ shareToken });
const identity = await viewer.request({ type: "share_identity" });
if (identity.share.label !== "smoke-viewers") fail("share identity");
const snap = await viewer.request({ type: "terminal_snapshot", session_id: sid });
let frame = snap.frame;
const sub = await viewer.request({ type: "subscribe_terminal", session_id: sid, max_hz: 30 });
let painted = false;
viewer.streams.set(sub.stream_id, (f) => {
  if (f.kind === Kind.FullFrame) frame = decodeFullFrame(f.payload).frame;
  else if (f.kind === Kind.FrameDelta) { try { applyDelta(frame, decodeFrameDelta(f.payload).delta); } catch (e) { fail(`delta: ${e.message}`); } }
  if (frame.rows.some((r) => r.cells.map((c) => c.grapheme).join("").includes("line "))) painted = true;
});
await sleep(1500);
if (!painted) fail("viewer never painted the stream");
const denied = await viewer.request({ type: "terminal_key", session_id: sid, input: { physical_key: "a", logical_key: "a", text: "a", modifiers: { shift: false, alt: false, control: false, super_key: false, caps_lock: false, num_lock: false }, consumed_modifiers: { shift: false, alt: false, control: false, super_key: false, caps_lock: false, num_lock: false }, action: "press", composing: false, unshifted_codepoint: "a" } }).then(() => "accepted", (e) => e.code);
const bare = await viewer.request({ type: "ping" }, { share_token: null }).then(() => "accepted", (e) => e.code);
const other = await viewer.request({ type: "list_devices" }).then(() => "accepted", (e) => e.code);
console.log(`   painted · key → ${denied} · token-less request → ${bare} · owner-only request → ${other}`);
if (denied !== "share_request_denied" || bare !== "share_token_required" || other !== "share_request_denied") fail("viewer limits");

step("3. the owner sees who is watching");
const viewers = host(`terminal-viewers ${sid}`);
const links = viewers.viewers.filter((v) => v.kind === "share");
console.log(`   ${viewers.viewers.length} viewer(s): ${viewers.viewers.map((v) => `${v.kind}/${v.role}/${v.label}`).join(", ")}`);
if (links.length !== 1 || links[0].label !== "smoke-viewers" || links[0].role !== "observer") fail("presence");

step("4. fifty viewers on one scrolling session, bytes per viewer measured");
const many = [];
for (let i = 0; i < 50; i++) many.push(await connect({ shareToken }));
for (const v of many) {
  const s = await v.request({ type: "subscribe_terminal", session_id: sid, max_hz: 30 });
  v.streams.set(s.stream_id, () => {});
  v.stats.bytes = 0; v.stats.frames = 0;
}
const seconds = 8;
await sleep(seconds * 1000);
const perViewer = many.map((v) => v.stats.bytes / seconds / 1024);
const avg = perViewer.reduce((a, b) => a + b, 0) / perViewer.length;
const max = Math.max(...perViewer);
const frames = many.reduce((a, v) => a + v.stats.frames, 0) / many.length / seconds;
const presence = host(`terminal-viewers ${sid}`).viewers.length;
console.log(`   viewers=${many.length} · avg ${avg.toFixed(1)} KB/s · max ${max.toFixed(1)} KB/s · ${frames.toFixed(1)} frames/s each · owner sees ${presence} viewers`);
if (presence < 51) fail("presence under load");
for (const v of many) v.ws.close();

step("5. revoking the share ends the viewer");
host(`share-revoke ${link.share_id}`);
await sleep(800);
const after = await viewer.request({ type: "ping" }).then(() => "accepted", (e) => e.code);
console.log(`   after revoke: ${after}${viewer.closed ? " · connection closed" : ""}`);
if (after === "accepted") fail("revoked share still served");
host(`terminal-kill ${sid}`);
console.log(`all stream steps pass · fan-out ${avg.toFixed(1)} KB/s per viewer at 30 Hz`);
process.exit(0);
