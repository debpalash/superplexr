// The browser's path, driven from node: fetch the page, open the WebSocket,
// pair with a code, list terminals, start a shell, subscribe, type, read the
// echo out of protobuf frames. Same modules the page loads.
import { readFileSync } from "node:fs";
import { execSync } from "node:child_process";
process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";
const [,, base, socket, bin] = process.argv;
const web = `${base.replace(/^https/, "https")}`;
const modOf = (file) => import(`file://${process.cwd()}/web/${file}`);
const { Kind, FrameReader, Sequencer, encodeJson, hello, json, uuid, PROTOCOL_VERSION } = await modOf("wire.js");
const { decodeFullFrame, decodeFrameDelta, applyDelta } = await modOf("frames.js");

const step = (s) => console.log(`▸ ${s}`);
const fail = (s) => { console.log(`   FAIL: ${s}`); process.exit(1); };
process.on("unhandledRejection", (e) => fail(`${e.code ?? ""} ${e.message ?? JSON.stringify(e)}`));

step("1. the page and its scripts are served with a strict CSP");
const page = await fetch(`${web}/`);
const csp = page.headers.get("content-security-policy") ?? "";
const html = await page.text();
if (!html.includes("ultraplexr") || !csp.includes("default-src 'self'")) fail(`page ${page.status} csp=${csp}`);
const js = await fetch(`${web}/app.js`);
if (!(js.headers.get("content-type") ?? "").startsWith("text/javascript")) fail("app.js content type");
const missing = await fetch(`${web}/../Cargo.toml`);
if (missing.status !== 404) fail(`traversal answered ${missing.status}`);
console.log(`   page ok · csp ok · 404 ok`);

function connectWs({ pairingCode = null, token = null, deviceId }) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${web.replace(/^http/, "ws")}/ws`);
    ws.binaryType = "arraybuffer";
    const reader = new FrameReader();
    const sequencer = new Sequencer();
    const waiters = new Map();
    const streams = new Map();
    const clientId = uuid();
    let welcome = null;
    ws.onopen = () => ws.send(encodeJson(Kind.Hello, 0, hello(clientId, deviceId, { deviceToken: token, pairingCode }), sequencer));
    ws.onmessage = (ev) => {
      reader.push(new Uint8Array(ev.data));
      for (const frame of reader.take()) {
        if (!welcome) {
          if (frame.kind === Kind.Welcome) { welcome = json(frame); resolve({ ws, welcome, request, streams }); }
          else if (frame.kind === Kind.Close) { reject(json(frame)); }
          continue;
        }
        if (frame.streamId === 0 && frame.kind === Kind.Response) {
          const r = json(frame); const w = waiters.get(r.request_id); waiters.delete(r.request_id);
          if (r.result.status === "success") w.resolve(r.result.body); else w.reject(r.result);
        } else if (frame.streamId !== 0) {
          streams.get(frame.streamId)?.(frame);
        }
      }
    };
    ws.onclose = (ev) => reject({ code: "closed", message: ev.reason });
    function request(action, extra = {}) {
      const id = uuid();
      ws.send(encodeJson(Kind.Request, 0, { version: PROTOCOL_VERSION, client_id: clientId, request_id: id, surface_id: null, control_epoch: null, ...extra, action }, sequencer));
      return new Promise((res, rej) => waiters.set(id, { resolve: res, reject: rej }));
    }
  });
}

step("2. an unpaired browser is refused at the handshake");
const deviceId = uuid();
try { await connectWs({ deviceId }); fail("was admitted"); } catch (e) { console.log(`   ${e.code}: ${e.message}`); if (e.code !== "gateway_unauthorized") fail("wrong refusal"); }

step("3. pair with a code from the host, get a token once");
const pairing = JSON.parse(execSync(`${bin} --socket ${socket} device-pair --label browser-test`).toString());
const paired = await connectWs({ deviceId, pairingCode: pairing.code });
if (!paired.welcome.device_token?.startsWith("spxd_")) fail("no token in welcome");
console.log(`   token minted (${paired.welcome.device_token.length} chars) · runtime ${paired.welcome.runtime_version}`);
paired.ws.close();

step("4. reconnect with the token, start a shell, subscribe, type, see the echo");
const live = await connectWs({ deviceId, token: paired.welcome.device_token });
const sid = uuid();
const surface = uuid();
const started = await live.request({ type: "start_terminal", spec: { session_id: sid, mission_id: null, run_id: null, program: "", args: [], cwd: "", environment_delta: {}, grid: { columns: 60, rows: 12 } } }, { surface_id: surface });
if (started.terminal.session_id !== sid) fail("start_terminal");
const claim = await live.request({ type: "claim_terminal_control", session_id: sid, force: false }, { surface_id: surface });
const epoch = claim.control_epoch;
const snap = await live.request({ type: "terminal_snapshot", session_id: sid });
let frame = snap.frame;
const sub = await live.request({ type: "subscribe_terminal", session_id: sid, max_hz: 60 });
let echoed = false;
const seen = { full: 0, delta: 0 };
live.streams.set(sub.stream_id, (f) => {
  if (f.kind === Kind.FullFrame) { frame = decodeFullFrame(f.payload).frame; seen.full++; }
  else if (f.kind === Kind.FrameDelta) { try { applyDelta(frame, decodeFrameDelta(f.payload).delta); seen.delta++; } catch (e) { fail(`delta: ${e.message}`); } }
  const text = frame.rows.map((r) => r.cells.map((c) => c.grapheme).join("")).join("\n");
  if (text.includes("WEB-ECHO-OK")) echoed = true;
});
await new Promise((r) => setTimeout(r, 800));
const key = (physical, text) => live.request({ type: "terminal_key", session_id: sid, input: { physical_key: physical, logical_key: physical, text, modifiers: { shift: false, alt: false, control: false, super_key: false, caps_lock: false, num_lock: false }, consumed_modifiers: { shift: false, alt: false, control: false, super_key: false, caps_lock: false, num_lock: false }, action: "press", composing: false, unshifted_codepoint: text } }, { surface_id: surface, control_epoch: epoch });
for (const ch of "echo WEB-ECH" ) await key(ch === " " ? "space" : ch, ch);
for (const ch of "O-OK") await key(ch, ch);
await key("enter", null);
for (let i = 0; i < 40 && !echoed; i++) await new Promise((r) => setTimeout(r, 100));
console.log(`   shell started · control epoch ${epoch} · frames full=${seen.full} delta=${seen.delta} · echoed=${echoed}`);
if (!echoed) { console.log(frame.rows.slice(0, 4).map((r) => r.cells.map((c) => c.grapheme).join("")).join("\n")); fail("no echo"); }

step("5. a wrong origin is refused before the upgrade");
const bad = await new Promise((resolve) => {
  const ws = new WebSocket(`${web.replace(/^http/, "ws")}/ws`, { headers: { Origin: "https://evil.example" } });
  ws.onerror = () => resolve("refused"); ws.onopen = () => resolve("opened");
});
console.log(`   ${bad}`); if (bad !== "refused") fail("origin");
await live.request({ type: "kill_terminal", session_id: sid }).catch(() => {});
live.ws.close();
console.log("all web steps pass");
process.exit(0);
