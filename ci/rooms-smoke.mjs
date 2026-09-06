// A room, driven from node: the owner's screen holds control, a viewer on a
// controller link raises a hand, the owner hands control over, the viewer
// types, hands it back, the owner types — two hand-offs and every
// keystroke lands. An observer link cannot take part; nobody can seize.
import { execSync } from "node:child_process";
process.env.NODE_TLS_REJECT_UNAUTHORIZED = "0";
const [,, base, socket, bin] = process.argv;
const modOf = (file) => import(`file://${process.cwd()}/web/${file}`);
const { Kind, FrameReader, Sequencer, encodeJson, hello, json, uuid, PROTOCOL_VERSION } = await modOf("wire.js");
const { decodeFullFrame, decodeFrameDelta, applyDelta } = await modOf("frames.js");

const step = (s) => console.log(`▸ ${s}`);
const fail = (s) => { console.log(`   FAIL: ${s}`); process.exit(1); };
process.on("unhandledRejection", (e) => fail(`${e.code ?? ""} ${e.message ?? JSON.stringify(e)}`));
const host = (args) => {
  try { return JSON.parse(execSync(`${bin} --socket ${socket} ${args}`, { stdio: ["ignore", "pipe", "pipe"] }).toString()); }
  catch (e) { fail(`host command \`${args}\`: ${e.stderr?.toString().trim() || e.message}`); }
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const NO_MODS = { shift: false, alt: false, control: false, super_key: false, caps_lock: false, num_lock: false };

function connect({ shareToken = null, deviceId = uuid(), token = null, pairingCode = null } = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${base.replace(/^http/, "ws")}/ws`);
    ws.binaryType = "arraybuffer";
    const reader = new FrameReader();
    const sequencer = new Sequencer();
    const waiters = new Map();
    const streams = new Map();
    const clientId = uuid();
    let welcome = null;
    ws.onopen = () => {
      const greeting = hello(clientId, deviceId, { deviceToken: token, pairingCode });
      greeting.share_token = shareToken;
      ws.send(encodeJson(Kind.Hello, 0, greeting, sequencer));
    };
    ws.onmessage = (ev) => {
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
        } else if (frame.streamId !== 0) streams.get(frame.streamId)?.(frame);
      }
    };
    ws.onclose = (ev) => reject({ code: "closed", message: ev.reason });
    function request(action, extra = {}) {
      const id = uuid();
      ws.send(encodeJson(Kind.Request, 0, { version: PROTOCOL_VERSION, client_id: clientId, request_id: id, surface_id: null, control_epoch: null, share_token: shareToken, ...extra, action }, sequencer));
      return new Promise((res, rej) => waiters.set(id, { resolve: res, reject: rej }));
    }
    const api = { ws, request, streams, clientId, get welcome() { return welcome; } };
  });
}

/** One participant's view of a session: control epoch, keys, the screen. */
function participant(conn, sid, surface, shareId) {
  const p = { conn, sid, surface, shareId, epoch: null, frame: null, me: { client_id: conn.clientId, surface_id: surface, share_id: shareId } };
  p.summary = async () => (await conn.request({ type: "list_terminals", include_archived: false })).terminals.find((t) => t.session_id === sid);
  p.act = (type, extra = {}) => conn.request({ type, session_id: sid, ...extra }, { surface_id: surface, control_epoch: p.epoch });
  p.key = async (ch) => { await p.act("terminal_key", { input: { physical_key: ch === "\r" ? "enter" : ch === " " ? "space" : ch, logical_key: ch, text: ch === "\r" ? null : ch, modifiers: NO_MODS, consumed_modifiers: NO_MODS, action: "press", composing: false, unshifted_codepoint: ch === "\r" ? null : ch } }); };
  p.type = async (text) => { for (const ch of text) await p.key(ch); };
  p.watch = async () => {
    p.frame = (await conn.request({ type: "terminal_snapshot", session_id: sid })).frame;
    const sub = await conn.request({ type: "subscribe_terminal", session_id: sid, max_hz: 60 });
    conn.streams.set(sub.stream_id, (f) => {
      if (f.kind === Kind.FullFrame) p.frame = decodeFullFrame(f.payload).frame;
      else if (f.kind === Kind.FrameDelta) { try { applyDelta(p.frame, decodeFrameDelta(f.payload).delta); } catch (e) { fail(`delta: ${e.message}`); } }
    });
  };
  p.screen = () => p.frame.rows.map((r) => r.cells.map((c) => c.grapheme).join("")).join("\n");
  return p;
}

step("1. the owner's screen, paired, starts a shell and holds control");
const pairing = host("device-pair --label owner-screen");
const ownerConn = await connect({ pairingCode: pairing.code });
const sid = uuid();
const ownerSurface = uuid();
await ownerConn.request({ type: "start_terminal", spec: { session_id: sid, mission_id: null, run_id: null, program: "/bin/sh", args: [], cwd: "/tmp", environment_delta: { PS1: "$ " }, grid: { columns: 60, rows: 10 } } }, { surface_id: ownerSurface });
const owner = participant(ownerConn, sid, ownerSurface, null);
owner.epoch = (await owner.summary()).control_epoch;
await owner.watch();
console.log(`   owner holds control · epoch ${owner.epoch}`);

step("2. a viewer on a controller link raises a hand; an observer link cannot");
const link = host(`stream ${sid} --controller --label pair-programmer`);
const observerLink = host(`stream ${sid} --label onlooker`);
const viewerConn = await connect({ shareToken: link.token });
const viewer = participant(viewerConn, sid, uuid(), link.share_id);
await viewer.watch();
const early = await viewer.act("accept_terminal_control").then(() => "accepted", (e) => e.message);
await viewer.act("request_terminal_control");
const onlooker = await connect({ shareToken: observerLink.token });
const denied = await onlooker.request({ type: "request_terminal_control", session_id: sid }).then(() => "accepted", (e) => e.code);
let summary = await owner.summary();
const hand = summary.control_requests.find((h) => h.share_id === link.share_id);
console.log(`   accept before any offer → "${early}" · observer hand → ${denied} · hands raised: ${summary.control_requests.map((h) => h.label).join(", ")}`);
if (!/not offered/.test(early) || denied !== "share_request_denied" || !hand) fail("hand raising");

step("3. hand-off one: the owner offers to that hand, the viewer accepts and types");
await owner.act("offer_terminal_control", { to: hand });
summary = await owner.summary();
if (!summary.control_offer || summary.control_offer.to?.share_id !== link.share_id) fail("offer not recorded");
const seize = await onlooker.request({ type: "accept_terminal_control", session_id: sid }).then(() => "accepted", (e) => e.code);
const accepted = await viewer.act("accept_terminal_control");
viewer.epoch = accepted.control_epoch;
const stale = await owner.type("x").then(() => "accepted", (e) => e.message);
await viewer.type("echo ab");
console.log(`   observer accept → ${seize} · viewer holds epoch ${viewer.epoch} · owner's key now → "${stale.slice(0, 60)}"`);
if (seize === "accepted" || stale === "accepted") fail("control leaked");

step("4. hand-off two: the viewer offers to anyone, the owner raises a hand and accepts");
await owner.act("request_terminal_control");
await viewer.act("offer_terminal_control", { to: null });
const back = await owner.act("accept_terminal_control");
owner.epoch = back.control_epoch;
await owner.type("cd\r");
await sleep(600);
const screen = owner.screen();
const viewerScreen = viewer.screen();
const line = screen.split("\n").find((l) => l.startsWith("abcd"));
console.log(`   owner holds epoch ${owner.epoch} · screen shows: ${line ?? "(no abcd line)"} · viewer sees the same: ${viewerScreen.includes("abcd")}`);
if (!line || !viewerScreen.includes("abcd")) { console.log(screen); fail("a keystroke was lost across the hand-offs"); }
summary = await owner.summary();
if (summary.control_offer || summary.control_requests.length) fail(`room state left behind: ${JSON.stringify({ offer: summary.control_offer, hands: summary.control_requests })}`);

step("5. presence names both participants");
const viewers = host(`terminal-viewers ${sid}`).viewers;
console.log(`   ${viewers.map((v) => `${v.kind}/${v.role}/${v.label}`).join(", ")}`);
if (!viewers.some((v) => v.label === "pair-programmer") || !viewers.some((v) => v.kind === "owner")) fail("presence");
await owner.act("kill_terminal").catch(() => {});
console.log("all room steps pass · two hand-offs, no keystroke lost");
process.exit(0);
