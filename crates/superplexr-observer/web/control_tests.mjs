import test from "node:test";
import assert from "node:assert/strict";
import {BrowserControl, bindTerminalInput, terminalKey} from "./control.mjs";

const settle = () => new Promise(resolve => setTimeout(resolve,0));
function harness(fetcher) {
  const failures=[];
  const client=new BrowserControl({headers:()=>({Authorization:"Bearer private"}),onChange:()=>{},onFailure:message=>failures.push(message),fetcher});
  client.attach("surface-a");
  return {client,failures};
}

test("ordered commands bind input to the acknowledged lease and never retry",async()=>{
  const sent=[];
  const {client,failures}=harness(async function(url,options){
    assert.equal(this,undefined,"native browser fetch must not receive the control client as its receiver");
    const body=JSON.parse(options.body);sent.push({url,body,headers:options.headers});
    return {ok:true,json:async()=>({sequence:body.sequence,lease:body.action.type==="release"?null:"lease-a"})};
  });
  assert.equal(client.send({type:"paste",text:"before claim"}),false);
  client.send({type:"claim"}); await settle();
  client.send({type:"paste",text:"first"});client.send({type:"paste",text:"second"});await settle();
  client.send({type:"release"});await settle();
  assert.deepEqual(sent.map(x=>x.body.sequence),[1,2,3,4]);
  assert.deepEqual(sent.map(x=>x.body.lease),[null,"lease-a","lease-a","lease-a"]);
  assert.ok(sent.every(x=>x.url==="/views/surface-a/commands"&&x.headers.Authorization==="Bearer private"));
  assert.equal(client.lease,null);assert.deepEqual(failures,[]);
});

test("failed or uncertain writes discard queued input and require reattachment",async()=>{
  let calls=0;
  const {client,failures}=harness(async(_url,options)=>{
    calls++;const body=JSON.parse(options.body);
    if(body.action.type==="claim") return {ok:true,json:async()=>({sequence:body.sequence,lease:"lease-a"})};
    throw Error("connection lost");
  });
  client.send({type:"claim"});await settle();
  client.send({type:"paste",text:"uncertain"});client.send({type:"paste",text:"must not replay"});await settle();
  assert.equal(calls,2);assert.equal(client.surface,null);assert.equal(client.queue.length,0);assert.equal(failures.length,1);
  client.attach("surface-b");assert.equal(client.lease,null);
  assert.equal(client.send({type:"paste",text:"no automatic reclaim"}),false);
});

test("an acknowledgement from a retired attachment cannot confer control",async()=>{
  let finish;
  const {client}=harness(()=>new Promise(resolve=>{finish=resolve;}));
  client.send({type:"claim"});client.attach("surface-b");
  finish({ok:true,json:async()=>({sequence:1,lease:"stale"})});await settle();
  assert.equal(client.surface,"surface-b");assert.equal(client.lease,null);assert.equal(client.sequence,0);
});

test("input queue is bounded and overflow fails closed",async()=>{
  const {client,failures}=harness((_url,{signal})=>new Promise((_,reject)=>signal.addEventListener("abort",()=>reject(Error("aborted")),{once:true})));client.lease="lease-a";
  for(let i=0;i<130;i++)client.send({type:"paste",text:"x"});
  assert.equal(client.surface,null);assert.equal(client.queue.length,0);assert.equal(failures.length,1);
  await settle();
});

test("keyboard mapping keeps browser shortcuts local and preserves terminal keys",()=>{
  const event=(key,extra={})=>({key,shiftKey:false,altKey:false,ctrlKey:false,metaKey:false,repeat:false,...extra});
  assert.equal(terminalKey(event("c",{metaKey:true})),null);
  assert.equal(terminalKey(event("V",{ctrlKey:true,shiftKey:true})),null);
  assert.equal(terminalKey(event("a",{isComposing:true})),null);
  assert.equal(terminalKey(event("ArrowUp")).physical_key,"up");
  assert.equal(terminalKey(event("c",{ctrlKey:true})).modifiers.control,true);
  assert.equal(terminalKey(event("界")).text,"界");
  assert.equal(terminalKey(event("İ")).unshifted_codepoint,null);
  assert.equal(terminalKey(event("F12")).physical_key,"f12");
});

test("DOM adapter suppresses duplicate character insertion after handled Enter",()=>{
  const input=Object.assign(new EventTarget(),{value:"",disabled:false});
  const keys=[],texts=[];
  const dispose=bindTerminalInput(input,{sendKey:key=>keys.push(key),pasteText:text=>texts.push(text),leaveInput:()=>{}});
  const emit=(type,fields={})=>input.dispatchEvent(Object.assign(new Event(type,{cancelable:true}),fields));
  emit("keydown",{key:"Enter",ctrlKey:false});
  assert.equal(emit("beforeinput",{inputType:"insertText",data:"\r"}),false);
  // Some drivers still send input after a cancelled beforeinput.
  input.value="\n";emit("input",{inputType:"insertText",data:"\r"});emit("keyup");
  assert.equal(keys.length,1);assert.equal(keys[0].physical_key,"enter");assert.deepEqual(texts,[]);
  input.value="text-only input";emit("input",{inputType:"insertText"});assert.deepEqual(texts,["text-only input"]);
  input.disabled=true;input.value="blocked";emit("input",{inputType:"insertText"});emit("keydown",{key:"x"});
  assert.equal(keys.length,1);assert.equal(texts.length,1);dispose();
});

test("composition is sent at commit and clipboard text uses confirmation path",()=>{
  const input=Object.assign(new EventTarget(),{value:"",disabled:false});
  const texts=[];let escaped=false;
  bindTerminalInput(input,{sendKey:()=>assert.fail("composition is not a physical key"),pasteText:text=>texts.push(text),leaveInput:()=>{escaped=true;}});
  const emit=(type,fields={})=>input.dispatchEvent(Object.assign(new Event(type,{cancelable:true}),fields));
  emit("compositionstart");input.value="界";emit("input",{isComposing:true,inputType:"insertCompositionText"});
  emit("compositionend",{data:"界"});emit("input",{inputType:"insertCompositionText",data:"界"});
  assert.deepEqual(texts,["界"]);
  emit("paste",{clipboardData:{getData:()=>"review before sending\n"}});
  assert.deepEqual(texts,["界","review before sending\n"]);
  emit("keydown",{key:"]",ctrlKey:true});assert.equal(escaped,true);
});
