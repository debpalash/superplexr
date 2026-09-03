// Replay, driven from node: a session that prints in three beats with a
// control change in the middle and then exits; its chapters; a replay at
// four times speed that lands the same screen in a quarter of the time;
// a replay from the control chapter that starts past the first beat; and
// the TUI player under a pty.
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

function connect({ pairingCode = null, deviceId = uuid() } = {}) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${base.replace(/^http/, "ws")}/ws`);
    ws.binaryType = "arraybuffer";
    const reader = new FrameReader(); const sequencer = new Sequencer();
    const waiters = new Map(); const streams = new Map(); const clientId = uuid();
    let welcome = null;
    ws.onopen = () => ws.send(encodeJson(Kind.Hello, 0, hello(clientId, deviceId, { pairingCode }), sequencer));
    ws.onmessage = (ev) => {
      reader.push(new Uint8Array(ev.data));
      for (const frame of reader.take()) {
        if (!welcome) { if (frame.kind === Kind.Welcome) { welcome = json(frame); resolve(api); } else if (frame.kind === Kind.Close) reject(json(frame)); continue; }
        if (frame.streamId === 0 && frame.kind === Kind.Response) {
          const r = json(frame); const w = waiters.get(r.request_id); waiters.delete(r.request_id);
          if (!w) continue; if (r.result.status === "success") w.resolve(r.result.body); else w.reject(r.result);
        } else if (frame.streamId !== 0) streams.get(frame.streamId)?.(frame);
      }
    };
    ws.onclose = (ev) => reject({ code: "closed", message: ev.reason });
    function request(action, extra = {}) {
      const id = uuid();
      ws.send(encodeJson(Kind.Request, 0, { version: PROTOCOL_VERSION, client_id: clientId, request_id: id, surface_id: null, control_epoch: null, ...extra, action }, sequencer));
      return new Promise((res, rej) => waiters.set(id, { resolve: res, reject: rej }));
    }
    const api = { ws, request, streams, clientId };
  });
}

/** Play a recording and collect what it shows and how long it took. */
function play(conn, sid, fromOffset, speedPercent) {
  return new Promise(async (resolve) => {
    let frame = null; let frames = 0; const t0 = Date.now(); let first = null;
    const sub = await conn.request({ type: "subscribe_terminal_replay", session_id: sid, from_offset: fromOffset, speed_percent: speedPercent, max_hz: 60 });
    conn.streams.set(sub.stream_id, (f) => {
      if (f.kind === Kind.FullFrame) { frame = decodeFullFrame(f.payload).frame; if (!first) first = frame.rows.map((r) => r.cells.map((c) => c.grapheme).join("")).join("\n"); }
      else if (f.kind === Kind.FrameDelta) { try { applyDelta(frame, decodeFrameDelta(f.payload).delta); } catch (e) { fail(`delta: ${e.message}`); } }
      else if (f.kind === Kind.Close) { resolve({ ms: Date.now() - t0, frames, first, screen: frame.rows.map((r) => r.cells.map((c) => c.grapheme).join("")).join("\n") }); return; }
      frames++;
    });
  });
}

step("1. record: three beats of output with a control change between them, then exit");
const pairing = host("device-pair --label replay-owner");
const owner = await connect({ pairingCode: pairing.code });
const sid = uuid();
const surface = uuid();
await owner.request({ type: "start_terminal", spec: { session_id: sid, mission_id: null, run_id: null, program: "/bin/sh", args: ["-c", "echo BEAT-ONE; sleep 0.6; echo BEAT-TWO; sleep 0.6; echo REPLAY-END; sleep 0.2"], cwd: "/tmp", environment_delta: {}, grid: { columns: 50, rows: 8 } } }, { surface_id: surface });
await sleep(650);
const epoch = (await owner.request({ type: "list_terminals", include_archived: false })).terminals.find((t) => t.session_id === sid).control_epoch;
await owner.request({ type: "release_terminal_control", session_id: sid }, { surface_id: surface, control_epoch: epoch });
await owner.request({ type: "claim_terminal_control", session_id: sid, force: false }, { surface_id: uuid() });
await sleep(1200);
const chapters = await owner.request({ type: "terminal_chapters", session_id: sid });
const kinds = chapters.chapters.map((c) => c.kind);
console.log(`   ${chapters.journal_bytes} journal bytes · ${chapters.duration_ms} ms recorded · chapters: ${chapters.chapters.map((c) => `${c.index}:${c.kind}@${c.journal_offset}`).join(" ")}`);
if (kinds[0] !== "started" || !kinds.includes("control") || kinds[kinds.length - 1] !== "exited") fail(`chapters ${kinds}`);
if (chapters.duration_ms < 1000) fail("timing was not recorded");

step("2. replay at ×4 from the start lands the same screen in about a quarter of the time");
const full = await play(owner, sid, 0, 400);
console.log(`   ${full.frames} frames in ${full.ms} ms · ends with REPLAY-END: ${full.screen.includes("REPLAY-END")} · BEAT-ONE first: ${full.first.includes("BEAT-ONE") || !full.first.includes("BEAT-TWO")}`);
if (!full.screen.includes("REPLAY-END") || full.ms > 900 || full.ms < 150) fail("pace");
if (full.first.includes("BEAT-TWO")) fail("replay from 0 started too late");

step("3. replay from the control chapter starts past the first beat");
const control = chapters.chapters.find((c) => c.kind === "control");
const later = await play(owner, sid, control.journal_offset, 800);
console.log(`   first frame has BEAT-ONE: ${later.first.includes("BEAT-ONE")} · ends with REPLAY-END: ${later.screen.includes("REPLAY-END")} · ${later.ms} ms`);
if (!later.first.includes("BEAT-ONE") || !later.screen.includes("REPLAY-END")) fail("chapter start");

step("4. the TUI player under a pty");
const out = execSync(`( sleep 3; printf 'q'; sleep 0.3 ) | script -q /dev/null ${bin} --socket ${socket} replay ${sid} --speed 8 2>&1 | cat`, { timeout: 20000 }).toString();
console.log(`   painted REPLAY-END: ${out.includes("REPLAY-END")} · said end of recording: ${out.includes("end of recording")}`);
if (!out.includes("REPLAY-END")) fail("tui replay");
console.log("all replay steps pass");
process.exit(0);
