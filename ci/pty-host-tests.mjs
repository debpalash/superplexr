import test from "node:test";
import assert from "node:assert/strict";
import {spawn} from "node:child_process";
import {fileURLToPath} from "node:url";
const helper=fileURLToPath(new URL("./pty-host.py",import.meta.url));

function start(script){
  const child=spawn("python3",[helper,"/bin/sh","-c",script],{stdio:["pipe","pipe","pipe"]});
  let text="";
  child.stdout.on("data",chunk=>text+=chunk);
  child.stderr.on("data",chunk=>text+=chunk);
  const done=new Promise((resolve,reject)=>{child.once("error",reject);child.once("close",(code,signal)=>resolve({code,signal}));});
  return {child,done,text:()=>text};
}

test("Node pipe gets a real sized controlling PTY with input forwarding",{timeout:5000},async()=>{
  const fixture=start('test -t 0 && test -t 1 && stty size; read -r value; printf "received:%s\\n" "$value"');
  try {
    fixture.child.stdin.write("fixture-input\n");
    const result=await fixture.done;
    assert.equal(result.code,0,fixture.text());
    assert.match(fixture.text(),/^ULTRAPLEXR_PTY_PID \d+\n/);
    assert.match(fixture.text(),/36 120/);
    assert.match(fixture.text(),/received:fixture-input/);
  } finally {fixture.child.kill("SIGTERM");}
});

test("stopping the host terminates and reaps its child",{timeout:5000},async()=>{
  const fixture=start('trap "exit 0" TERM; printf ready; while :; do read -r value; done');
  try {
    for(let i=0;!fixture.text().includes("ready")&&i<100;i++)await new Promise(r=>setTimeout(r,10));
    const match=/^ULTRAPLEXR_PTY_PID (\d+)\n/.exec(fixture.text());
    assert.ok(match,fixture.text());
    fixture.child.kill("SIGTERM");
    await fixture.done;
    assert.throws(()=>process.kill(Number(match[1]),0),{code:"ESRCH"});
  } finally {fixture.child.kill("SIGTERM");}
});
