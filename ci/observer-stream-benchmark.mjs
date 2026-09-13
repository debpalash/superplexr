#!/usr/bin/env node
// Reproducible local HTTP-delivery baseline; does not measure browser paint.
// Run after cargo build -p superplexr-server -p superplexr-cli -p superplexr-observer.
import {spawn, execFile} from "node:child_process";
import {promisify} from "node:util";
import {mkdtemp, writeFile, rm} from "node:fs/promises";
import {join, resolve} from "node:path";
import os from "node:os";
import {performance} from "node:perf_hooks";
import {readEvents} from "../crates/superplexr-observer/web/stream.mjs";
const exec = promisify(execFile), sleep = ms => new Promise(r=>setTimeout(r,ms));
const binaries = resolve(process.env.SUPERPLEXR_BINARY_DIR || "target/debug");
const root = await mkdtemp("/tmp/up-stream-bench-");
const socket = join(root,"s"), children = [];
let controller, streamTask, metadataTimer, metadataError;
async function cli(...args) {
  const {stdout} = await exec(join(binaries,"superplexr"), ["--socket",socket,...args], {timeout:10000});
  return JSON.parse(stdout);
}
function start(name,args) {
  const child=spawn(join(binaries,name),args,{stdio:["ignore","pipe","pipe"]});
  let output=""; child.stdout.on("data",c=>output+=c); child.stderr.on("data",c=>output+=c);
  children.push(child); return {child,output:()=>output};
}
async function until(predicate, ms=10000) {
  const deadline=performance.now()+ms;
  while (!(await predicate())) { if(performance.now()>deadline) throw new Error("Deadline exceeded"); await sleep(25); }
}
function cpuSeconds(time) {
  const [s,m,h] = time.split(":").reverse().map(Number);
  return s+(m||0)*60+(h||0)*3600;
}
async function sample(pid) {
  const {stdout}=await exec("ps",["-p",String(pid),"-o","rss=,time="]);
  const [rss,time]=stdout.trim().split(/\s+/);
  return {rss_mib:Number(rss)/1024,cpu_seconds:cpuSeconds(time)};
}
try {
  const runtime=start("superplexr-server",["--socket",socket,"--state-dir",join(root,"state")]);
  await until(()=>runtime.output().includes("runtime listening"));
  const terminal=await cli("terminal-new","--program","/bin/cat","--columns","80","--rows","24");
  const id=terminal.terminal.session_id;
  const share=await cli("share-create","benchmark","--role","observer","--session",id,"--expires-in-seconds","120");
  const tokenFile=join(root,"observer.token");
  await writeFile(tokenFile,share.token+"\n",{mode:0o600});
  const observer=start("superplexr-observer",["--socket",socket,"--share-token-file",tokenFile]);
  await until(()=>observer.output().includes("http://"));
  const url=new URL(observer.output().match(/http:\/\/\S+/)[0]);
  const key=new URLSearchParams(url.hash.slice(1)).get("access");
  controller=new AbortController();
  // Include the browser's session-list cadence, but not the browser process.
  metadataTimer=setInterval(()=>{
    fetch(url.origin+"/sessions",{headers:{Authorization:"Bearer "+key},signal:controller.signal})
      .then(async response=>{ if(!response.ok) throw new Error("Metadata refused: "+response.status); await response.json(); })
      .catch(error=>{ if(!controller.signal.aborted) metadataError=error; });
  },2000);
  const response=await fetch(url.origin+"/sessions/"+id+"/events",{headers:{Authorization:"Bearer "+key},signal:controller.signal});
  if(!response.ok) throw new Error("SSE refused: "+response.status);
  let frames=0, text="", sequence=0;
  streamTask=readEvents(response.body,({event,data})=>{
    if(event==="frame"){ const frame=JSON.parse(data); frames++; text=frame.text; sequence=frame.sequence; }
  },controller.signal).catch(error=>{ if(!controller.signal.aborted) throw error; });
  await until(()=>frames>0);
  await sleep(1000);
  const idleFrames=frames;
  const idleStart=performance.now(), native0=await sample(runtime.child.pid), web0=await sample(observer.child.pid);
  await sleep(5000);
  const native1=await sample(runtime.child.pid), web1=await sample(observer.child.pid);
  const idleSeconds=(performance.now()-idleStart)/1000;
  const framesDuringIdle=frames-idleFrames;
  const timings=[], streamStart=performance.now();
  for(let i=0;i<50;i++){
    const marker="LATENCY-"+String(i).padStart(3,"0");
    const started=performance.now();
    await cli("terminal-write",id,marker+"\n");
    await until(()=>text.includes(marker),5000);
    timings.push(performance.now()-started);
    await sleep(20);
  }
  const native2=await sample(runtime.child.pid), web2=await sample(observer.child.pid);
  const streamSeconds=(performance.now()-streamStart)/1000;
  timings.sort((a,b)=>a-b);
  const state=await cli("terminal-list");
  const result={
    benchmark:"observer_event_stream", date:new Date().toISOString(), platform:os.platform(), architecture:os.arch(),
    cpu:os.cpus()[0].model, logical_cpus:os.cpus().length, build:"optimized development (not release)",
    workload:{sessions:1,grid:"80x24",markers:50,metadata_poll_seconds:2,idle_seconds:idleSeconds,stream_seconds:streamSeconds},
    latency_ms:{p50:timings[24],p95:timings[47],max:timings[49],includes:"CLI process launch + IPC + PTY echo + SSE delivery; 25ms observation granularity"},
    idle:{frames:framesDuringIdle, runtime_rss_mib:native1.rss_mib,observer_rss_mib:web1.rss_mib,
      runtime_cpu_percent:100*(native1.cpu_seconds-native0.cpu_seconds)/idleSeconds,
      observer_cpu_percent:100*(web1.cpu_seconds-web0.cpu_seconds)/idleSeconds},
    streaming:{frames:frames-idleFrames,sequence,runtime_rss_mib:native2.rss_mib,observer_rss_mib:web2.rss_mib,
      runtime_cpu_percent:100*(native2.cpu_seconds-native1.cpu_seconds)/streamSeconds,
      observer_cpu_percent:100*(web2.cpu_seconds-web1.cpu_seconds)/streamSeconds},
    terminal_count:state.terminals.length
  };
  if(framesDuringIdle!==0) throw new Error("Idle observer sent unexpected frame traffic");
  if(metadataError) throw metadataError;
  console.log(JSON.stringify(result,null,2));
  await cli("share-revoke",share.share.share_id);
  await cli("terminal-kill",id);
} finally {
  clearInterval(metadataTimer);
  controller?.abort();
  await streamTask;
  for(const child of children.reverse()){
    child.kill("SIGINT");
    await Promise.race([new Promise(resolve=>child.once("exit",resolve)),sleep(1000)]);
    if(child.exitCode===null && child.signalCode===null) child.kill("SIGKILL");
  }
  await rm(root,{recursive:true,force:true});
}
