#!/usr/bin/env node
// Isolated real-runtime/shared-client resource evidence. Browser means an HTTP
// consumer and gateway here, not Chromium/Safari rendering or browser-process RSS.
import {spawn, execFile} from "node:child_process";
import {promisify} from "node:util";
import {mkdtemp, writeFile, readFile, rm, stat, access} from "node:fs/promises";
import {createReadStream, constants} from "node:fs";
import {createHash} from "node:crypto";
import {join, resolve} from "node:path";
import {parseArgs} from "node:util";
import {fileURLToPath} from "node:url";
import os from "node:os";
import {performance} from "node:perf_hooks";
import {readEvents} from "../crates/ultraplexr-observer/web/stream.mjs";
import {parseSamples, summarize} from "./resource-samples.mjs";

const {values: options} = parseArgs({options:{
  "binary-dir":{type:"string",default:"target/release"},
  "idle-seconds":{type:"string",default:"300"},
  "sample-seconds":{type:"string",default:"10"},
  "history-lines":{type:"string",default:"100000"},
  "without-desktop":{type:"boolean",default:false},
  output:{type:"string"},
}});
const duration = Number(options["idle-seconds"]), interval = Number(options["sample-seconds"]);
const history = Number(options["history-lines"]), binaries = resolve(options["binary-dir"]);
if (!Number.isInteger(duration) || duration < 1 || duration > 3600
  || !Number.isInteger(interval) || interval < 1 || interval > 30
  || !Number.isInteger(history) || history < 0 || history > 100000) throw new Error("Invalid bounded benchmark options");
if (!['darwin','linux'].includes(os.platform())) throw new Error("This benchmark requires Unix ps/PTYs");
const names = ["ultraplexr-server","ultraplexr","ultraplexr-observer","ultraplexr-tui"];
if (!options["without-desktop"]) names.push("ultraplexr-desktop");
const binaryMetadata = {};
for (const name of names) {
  const path = join(binaries,name);
  await access(path,constants.X_OK);
  const hash = createHash("sha256");
  for await (const bytes of createReadStream(path)) hash.update(bytes);
  binaryMetadata[name] = {path,bytes:(await stat(path)).size,sha256:hash.digest("hex")};
}
const exec = promisify(execFile), sleep = ms => new Promise(r=>setTimeout(r,ms));
const root = await mkdtemp("/tmp/up-shared-budget-");
const socket = join(root,"s"), children = [], sessions = [];
let abort, streamTask, metadataTask, streamError, metadataError, tui, tuiPid;
const progress = text => process.stderr.write(text+"\n");
async function cli(...args) {
  const {stdout} = await exec(join(binaries,"ultraplexr"),["--socket",socket,...args],{timeout:30000,maxBuffer:2*1024*1024});
  return JSON.parse(stdout);
}
function start(name,args,executable=join(binaries,name)) {
  const child = spawn(executable,args,{stdio:["pipe","pipe","pipe"],env:{...process.env,TERM:"xterm-256color"}});
  const record = {name,child,bytes:0,text:"",ended:false,ptyPid:null};
  record.closed = new Promise(resolve=>{
    child.once("close",()=>{record.ended=true;resolve();});
    child.once("error",()=>{record.ended=true;resolve();});
  });
  for (const pipe of [child.stdout,child.stderr]) pipe.on("data",bytes=>{
    record.bytes += bytes.length;
    const text=record.text+bytes.toString();
    record.ptyPid ||= Number(text.match(/^ULTRAPLEXR_PTY_PID (\d+)\n/m)?.[1])||null;
    record.text = text.slice(-32768);
  });
  child.stdin.on("error",()=>{}); // An exiting PTY may close before detach arrives.
  children.push(record); return record;
}
async function until(predicate,timeout=30000) {
  const end = performance.now()+timeout;
  while (!(await predicate())) {
    if (performance.now()>end) throw new Error("Benchmark stage deadline exceeded");
    await sleep(50);
  }
}
async function sample(measured) {
  const pids=Object.values(measured);
  const {stdout}=await exec("ps",["-p",pids.join(","),"-o","pid=,rss=,time="],{timeout:5000});
  return {monotonic_ms:performance.now(),processes:parseSamples(stdout,pids)};
}
async function verifyReference() {
  for (const session of sessions) {
    const {capture}=await cli("terminal-capture",session.id);
    if(capture.frame.grid.columns!==80||capture.frame.grid.rows!==24)
      throw new Error("Client changed the reference terminal grid");
    if(capture.terminal.controller_client_id)
      throw new Error("Idle observer fixture unexpectedly acquired Control");
    if(history&&(await cli("terminal-search",session.id,"H000000","--limit","1")).matches.length!==1)
      throw new Error("Client attachment lost reference history");
  }
}
async function stop(record) {
  if (record.ended) return;
  record.child.kill("SIGTERM");
  await Promise.race([record.closed,sleep(2000)]);
  if (!record.ended) {record.child.kill("SIGKILL");await record.closed;}
}
let result;
try {
  progress("Starting isolated runtime and 12 history-loaded Sessions");
  const runtime=start("ultraplexr-server",["--socket",socket,"--state-dir",join(root,"state")]);
  await until(()=>{if(runtime.ended)throw new Error("Fixture runtime exited");return runtime.text.includes("runtime listening");});
  // Exactly 100000 scrollback rows plus the live viewport. Lines are short ASCII,
  // not images/wide-cell worst cases; report this workload rather than generalize.
  const seed=join(root,"history.txt");
  const lineCount=history ? history+24 : 1;
  // No trailing newline: a blank final live row would exceed the history limit
  // by one and trigger legitimate whole-page pruning before measurement.
  await writeFile(seed,Array.from({length:lineCount},(_,i)=>`H${String(i).padStart(6,"0")} fixed retained history sample`).join("\n"),{mode:0o600});
  for(let i=0;i<12;i++) {
    const value=await cli("terminal-new","--program","/bin/sh","--cwd",root,"--columns","80","--rows","24","--","-c",'cat "$1"; exec /bin/cat',"history",seed);
    const terminal=value.terminal;
    sessions.push({id:terminal.session_id,pid:terminal.process_id});
    const last=`H${String(lineCount-1).padStart(6,"0")}`;
    await until(async()=> (await cli("terminal-search",terminal.session_id,last,"--limit","1")).matches.length===1);
    const point=await sample({runtime:runtime.child.pid});
    if(point.processes[runtime.child.pid].rss_mib>2048)throw new Error("Fixture runtime exceeded 2 GiB seeding safety cap");
  }
  if (history) {
    for (const session of sessions) {
      const found=await cli("terminal-search",session.id,"H000000","--limit","1");
      if(found.matches.length!==1) throw new Error("Deep retained-history marker missing");
    }
  }
  await cli("session-group-create","resource-fixture",...sessions.flatMap(s=>["--session",s.id]));
  const share=await cli("share-create","resource-fixture","--role","observer",...sessions.flatMap(s=>["--session",s.id]),"--expires-in-seconds",String(duration+300));
  const token=join(root,"observer.token");
  await writeFile(token,share.token+"\n",{mode:0o600});
  const gateway=start("ultraplexr-observer",["--socket",socket,"--share-token-file",token]);
  await until(()=>{if(gateway.ended)throw new Error("Fixture gateway exited");return gateway.text.includes("http://");});
  const url=new URL(gateway.text.match(/http:\/\/\S+/)[0]);
  const secret=new URLSearchParams(url.hash.slice(1)).get("access");
  abort=new AbortController();
  const headers={Authorization:"Bearer "+secret};
  const response=await fetch(`${url.origin}/sessions/${sessions[0].id}/events`,{headers,signal:abort.signal});
  if(!response.ok) throw new Error(`Event feed refused: ${response.status}`);
  let frames=0,frameText="";
  streamTask=readEvents(response.body,({event,data})=>{
    if(event==="frame"){frames++;frameText=JSON.parse(data).text;}
  },abort.signal).catch(error=>{if(!abort.signal.aborted)streamError=error;});
  metadataTask=(async()=>{
    while(!abort.signal.aborted){
      await sleep(2000); if(abort.signal.aborted)break;
      const response=await fetch(url.origin+"/sessions",{headers,signal:AbortSignal.any([abort.signal,AbortSignal.timeout(5000)])});
      if(!response.ok)throw new Error(`Metadata refused: ${response.status}`);
      await response.json();
    }
  })().catch(error=>{if(!abort.signal.aborted)metadataError=error;});
  const tuiArgs=[join(binaries,"ultraplexr-tui"),"--socket",socket,sessions[0].id];
  tui=start("tui-pty",[fileURLToPath(new URL("./pty-host.py",import.meta.url)),...tuiArgs],"python3");
  await until(()=>{if(tui.ended)throw new Error("TUI PTY exited: "+JSON.stringify(tui.text));tuiPid=tui.ptyPid;return tuiPid&&tui.text.includes("OBSERVE");});
  const measured={runtime:runtime.child.pid,gateway:gateway.child.pid,tui:tuiPid};
  if(!options["without-desktop"]){
    // Owner clients acquire Control and resize to window geometry. A real
    // Observer Share keeps the fixed reference grid authoritative in the runtime.
    const desktop=start("ultraplexr-desktop",["--connect-only","--socket",socket,"--state-dir",join(root,"state"),"--share-token-file",token,"--runtime-label","RESOURCE QA"]);
    measured.desktop=desktop.child.pid;
    await until(async()=>{
      if(desktop.ended)throw new Error("Fixture desktop exited");
      try{const text=await readFile(join(root,"state/workspaces.json"),"utf8");return sessions.every(s=>text.includes(s.id));}catch{return false;}
    });
  }
  await until(()=>frames>0);
  await verifyReference();
  await sleep(10000);
  const idleFrames=frames,idleTuiBytes=tui.bytes;
  const samples=[await sample(measured)];
  progress(`Measuring ${duration}s idle window; desktop=${!!measured.desktop}, history=${history} rows/Session`);
  const end=performance.now()+duration*1000;
  let announced=0;
  while(performance.now()<end){
    await sleep(Math.min(interval*1000,Math.max(0,end-performance.now())));
    const point=await sample(measured);samples.push(point);
    if(streamError||metadataError)throw new Error("Browser attachment failed during measurement");
    const total=Object.values(point.processes).reduce((sum,p)=>sum+p.rss_mib,0);
    if(total>2048)throw new Error("Fixture exceeded 2 GiB measurement safety cap");
    const elapsed=Math.floor((point.monotonic_ms-samples[0].monotonic_ms)/1000);
    if(elapsed>=announced+30){progress(`Idle sample ${elapsed}/${duration}s; combined sampled RSS ${total.toFixed(1)} MiB`);announced=elapsed;}
  }
  const idle=summarize(samples,measured);
  const idleFrameCount=frames-idleFrames,idleTuiCount=tui.bytes-idleTuiBytes;
  // Outside the CPU sample: scanning history itself is not an idle workload.
  await verifyReference();
  const latency=[];
  for(let i=0;i<20;i++){
    const marker=`SHARED-LATENCY-${String(i).padStart(3,"0")}`;
    const start=performance.now();
    await cli("--force-control","terminal-write",sessions[0].id,marker+"\n");
    await until(()=>frameText.includes(marker),5000);
    latency.push(performance.now()-start);
  }
  const listed=(await cli("terminal-list")).terminals;
  if(listed.length!==sessions.length||sessions.some(s=>!listed.some(t=>t.session_id===s.id&&t.process_id===s.pid&&t.status==="running")))throw new Error("Shared clients changed Session or process identity");
  const runtimeDesktopPeak=Math.max(...samples.map(s=>s.processes[measured.runtime].rss_mib+(measured.desktop?s.processes[measured.desktop].rss_mib:0)));
  latency.sort((a,b)=>a-b);
  result={
    benchmark:"shared_client_resources",version:2,date:new Date().toISOString(),platform:os.platform(),architecture:os.arch(),
    cpu:os.cpus()[0].model,logical_cpus:os.cpus().length,host_memory_mib:os.totalmem()/1048576,
    binaries:binaryMetadata,build_profile:"provided binaries; reproduce using documented --release build command",
    workload:{sessions:12,grid:"80x24",history_rows_requested:history,seed_lines:lineCount,history_marker_verified:history?"H000000":null,
      desktop:!!measured.desktop,desktop_role:measured.desktop?"observer":null,desktop_visible_surfaces:measured.desktop?1:0,
      reference_verified_before_and_after_idle:true,tui_panes:1,browser_event_feeds:1,browser_metadata_poll_seconds:2,warmup_seconds:10},
    idle:{...idle,unexpected_browser_frames:idleFrameCount,tui_bytes_during_idle:idleTuiCount,runtime_plus_desktop_peak_rss_mib:runtimeDesktopPeak},
    samples,latency_ms:{p50:latency[9],p95:latency[18],max:latency[19],includes:"CLI startup + IPC + PTY echo + HTTP SSE, with 50ms observation granularity; not input-to-pixel"},
    identity_preserved:true,
    limits:["Browser process/compositor/outer terminal are not measured","Short ASCII history, no Kitty images or wide-cell worst cases","Single host/sample, not cross-platform certification","RSS is sampled, not an allocation high-water mark","CPU uses ps counters; resolution is platform dependent"]
  };
  result.gate_observations={
    five_minute_sample:idle.seconds>=300,
    runtime_idle_cpu:idle.processes.runtime.cpu_percent_one_core<=0.5,
    desktop_idle_cpu:measured.desktop?idle.processes.desktop.cpu_percent_one_core<=1:null,
    runtime_plus_desktop_rss:measured.desktop&&history===100000?runtimeDesktopPeak<=600:null,
    idle_browser_frames:idleFrameCount===0,
    idle_tui_output:idleTuiCount===0,
  };
  await cli("share-revoke",share.share.share_id);
} finally {
  abort?.abort();
  await Promise.all([streamTask,metadataTask]);
  if(tui&&!tui.ended){
    tui.child.stdin.write("\x1dd");
    await Promise.race([tui.closed,sleep(2000)]);
    if(!tui.ended)await stop(tui);
  }
  for(const session of sessions){try{await cli("--force-control","terminal-kill",session.id);}catch{}}
  for(const record of children.reverse())await stop(record);
  await rm(root,{recursive:true,force:true});
}
if(result){
  const json=JSON.stringify(result,null,2)+"\n";
  if(options.output)await writeFile(resolve(options.output),json,{flag:"wx",mode:0o600});
  else process.stdout.write(json);
  progress(`Resource sample complete; observations: ${JSON.stringify(result.gate_observations)}`);
}
