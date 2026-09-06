#!/usr/bin/env node
// Disposable real-runtime fixture for rendered browser-search acceptance.
// Only this process's children, Sessions and generated directory are retired.
import {spawn, execFile} from "node:child_process";
import {mkdtemp, writeFile, rm} from "node:fs/promises";
import {promisify} from "node:util";
import {resolve, join} from "node:path";
const exec = promisify(execFile), root = await mkdtemp("/tmp/up-browser-search-");
const binaries = resolve("target/release"), socket = join(root,"s"), children = [];
let sessionId;
const sleep = ms => new Promise(done => setTimeout(done, ms));
async function cli(...args) {
  return JSON.parse((await exec(join(binaries,"ultraplexr"), ["--socket",socket,...args], {timeout:15000})).stdout);
}
function start(name, args) {
  const child = spawn(join(binaries,name), args, {stdio:["ignore","pipe","pipe"]});
  const record = {child, text:"", ended:false};
  record.closed = new Promise(done => {
    child.once("close", () => { record.ended = true; done(); });
    child.once("error", () => { record.ended = true; done(); });
  });
  for (const pipe of [child.stdout,child.stderr]) pipe.on("data", data => { record.text = (record.text + data).slice(-32768); });
  children.push(record); return record;
}
async function until(check) {
  const deadline = Date.now() + 15000;
  while (!await check()) { if (Date.now() > deadline) throw new Error("Fixture startup timeout"); await sleep(50); }
}
try {
  const runtime = start("ultraplexr-server", ["--socket",socket,"--state-dir",join(root,"state")]);
  await until(() => { if (runtime.ended) throw new Error(runtime.text); return runtime.text.includes("runtime listening"); });
  const {terminal} = await cli("terminal-new", "--program","/bin/sh","--cwd",root,"--columns","80","--rows","24","--","-c",
    "stty -echo; awk 'BEGIN { for(i=0;i<1200;i++) printf \"needle %04d\\n\",i; print \"needle <script>not executable</script> 界\"; print \"READY\" }'; exec cat");
  sessionId = terminal.session_id;
  await until(async () => (await cli("terminal-search",terminal.session_id,"READY","--limit","1")).matches.length === 1);
  const share = await cli("share-create","browser search QA","--role","controller","--session",terminal.session_id,"--expires-in-seconds","1800");
  const token = join(root,"share.token");
  await writeFile(token, share.token + "\n", {mode:0o600});
  const gateway = start("ultraplexr-observer", ["--socket",socket,"--share-token-file",token,"--allow-control"]);
  await until(() => { if (gateway.ended) throw new Error(gateway.text); return gateway.text.includes("http://"); });
  process.stdout.write(JSON.stringify({url:gateway.text.match(/http:\/\/\S+/)[0], socket, session:terminal.session_id, share:share.share.share_id, runtime_pid:runtime.child.pid, gateway_pid:gateway.child.pid}) + "\n");
  // Enter, stdin closure or termination ends only this disposable fixture.
  await new Promise(done => {
    process.stdin.once("data",done); process.stdin.once("end",done); process.stdin.resume();
    process.once("SIGINT",done); process.once("SIGTERM",done);
  });
} finally {
  if (sessionId) { try { await cli("--force-control","terminal-kill",sessionId); } catch {} }
  for (const record of children.reverse()) {
    if (record.ended) continue;
    record.child.kill("SIGTERM");
    await Promise.race([record.closed,sleep(2000)]);
    if (!record.ended) { record.child.kill("SIGKILL"); await record.closed; }
  }
  await rm(root,{recursive:true,force:true});
  process.stdin.pause();
}
