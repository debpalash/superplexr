import test from "node:test";
import assert from "node:assert/strict";
import {FrameDecoder,FrameContinuityError} from "./frames.mjs";
import {TerminalDisplay} from "./terminal.mjs";
import {domFixture} from "./dom-fixture.mjs";

const color = (red=200,green=200,blue=200) => ({red,green,blue});
const style = extra => ({foreground:color(),background:color(0,0,0),underline:"none",...extra});
const run = (text,columns=text.length,style=0) => ({text,columns,style});
const frame = extra => ({revision:"9007199254740993",sequence:9007199254740992,rows:2,title:"title",directory:"/project",status:"running",text:"one\ntwo",
  display:{columns:8,rows:[[run("one")],[run("two")]],styles:[style()],foreground:color(),background:color(0,0,0),cursor:null},...extra});
const delta = extra => ({base_revision:"9007199254740993",revision:"9007199254740994",sequence:9007199254740994,rows:2,title:"changed",directory:null,status:"running",cursor:null,
  changed:[{index:1,text:"new",runs:[run("new")]}],...extra});
const accept = (decoder,event,value) => decoder.accept(event,JSON.stringify(value));

test("row deltas preserve exact u64 revision identity and update text, style runs and metadata atomically", () => {
  const decoder = new FrameDecoder(); const base = accept(decoder,"frame",frame());
  const next = accept(decoder,"frame-delta",delta());
  assert.equal(next.revision,"9007199254740994"); assert.equal(next.text,"one\nnew"); assert.equal(next.title,"changed");
  assert.equal(next.display.rows[0],base.display.rows[0]); assert.equal(next.display.rows[1][0].text,"new");
  assert.equal(base.text,"one\ntwo"); assert.equal(base.display.rows[1][0].text,"two");
});

test("invalid bases, regressions, duplicate rows and out-of-bounds styles cannot change the canonical frame", () => {
  for (const patch of [delta({base_revision:"9007199254740992"}),delta({revision:"9007199254740993"}),delta({revision:"18446744073709551616"}),
    delta({revision:9007199254740994}),delta({rows:3}),delta({sequence:-1}),delta({changed:[{index:2,text:"x",runs:[run("x")]}]}),
    delta({changed:[{index:0,text:"valid",runs:[run("valid")]},{index:0,text:"duplicate",runs:[run("duplicate",8)]}]}),
    delta({changed:[{index:0,text:"valid",runs:[run("valid")]},{index:1,text:"bad",runs:[run("bad",3,9)]}]}),
    delta({changed:[{index:0,text:"bad\nline",runs:[run("bad")]}]}),delta({changed:[{index:0,text:"too wide",runs:[run("too wide",9)]}]})]) {
    const decoder = new FrameDecoder(); const base = accept(decoder,"frame",frame());
    assert.throws(() => accept(decoder,"frame-delta",patch),FrameContinuityError); assert.equal(decoder.current,base);
    assert.equal(base.text,"one\ntwo"); assert.equal(base.display.rows[0][0].text,"one");
  }
});

test("reset requires a fresh base and text-only legacy frames cannot admit row deltas", () => {
  const decoder = new FrameDecoder(); accept(decoder,"frame",frame()); decoder.reset();
  assert.equal(decoder.current,null); assert.throws(() => accept(decoder,"frame-delta",delta()),FrameContinuityError);
  assert.equal(accept(decoder,"frame",{text:"legacy",rows:2}).text,"legacy");
  assert.throws(() => accept(decoder,"frame-delta",delta()),FrameContinuityError);
  accept(decoder,"frame",frame()); assert.equal(accept(decoder,"frame-delta",delta()).text,"one\nnew");
});

test("styled rows reuse unchanged DOM and repaint existing rows when the style table changes", t => {
  const {document} = domFixture(), previous = globalThis.document; globalThis.document = document;
  t.after(() => { if (previous === undefined) delete globalThis.document; else globalThis.document = previous; });
  const output = document.createElement("pre"), display = new TerminalDisplay(output), decoder = new FrameDecoder();
  const base = accept(decoder,"frame",frame()); assert.equal(display.render(base),true);
  const grid = output.firstElementChild, firstRow = grid.firstElementChild, firstRun = firstRow.firstElementChild, secondRun = grid.lastElementChild.firstElementChild;
  assert.equal(display.render(accept(decoder,"frame-delta",delta())),true);
  assert.equal(output.firstElementChild,grid); assert.equal(grid.firstElementChild,firstRow); assert.equal(firstRow.firstElementChild,firstRun);
  assert.notEqual(grid.lastElementChild.firstElementChild,secondRun); assert.equal(grid.lastElementChild.textContent,"new");
  const changed = frame(); changed.display.styles = [style({bold:true,foreground:color(255,0,0)})];
  assert.equal(display.render(changed),true); assert.equal(grid.firstElementChild,firstRow);
  assert.notEqual(firstRow.firstElementChild,firstRun); assert.equal(firstRow.firstElementChild.style.color,"rgb(255, 0, 0)");
  assert.equal(firstRow.firstElementChild.firstElementChild.style.fontWeight,"700");
});

test("wide glyphs, concealed styles and cursor rendering keep text literal and the grid bounded", t => {
  const {document} = domFixture(), previous = globalThis.document; globalThis.document = document;
  t.after(() => { if (previous === undefined) delete globalThis.document; else globalThis.document = previous; });
  const output = document.createElement("pre"), display = new TerminalDisplay(output), value = frame({text:"界<img>"});
  value.display.rows = [[run("界",2),run("<img>",5)]];
  value.display.styles = [style({invisible:true,underline:"curly",strikethrough:true})];
  value.display.cursor = {row:0,column:1,shape:"bar"};
  assert.equal(display.render(value),true); assert.equal(display.text,"界<img>");
  const row = output.firstElementChild.firstElementChild;
  assert.equal(row.firstElementChild.style.width,"2ch"); assert.equal(row.textContent,"界<img>");
  assert.equal(row.firstElementChild.style.color,"transparent");
  assert.equal(row.firstElementChild.firstElementChild.style.textDecorationLine,"none");
  assert.equal(row.classList.contains("terminal-cursor-row"),true); assert.equal(row.dataset.cursor,"bar");
  assert.equal(display.render(value,{cursor:false}),true); assert.equal(row.classList.contains("terminal-cursor-row"),false);
});

test("malformed styles and later invalid rows discard partial grids and recover on a valid full frame", t => {
  const {document} = domFixture(), previous = globalThis.document; globalThis.document = document;
  t.after(() => { if (previous === undefined) delete globalThis.document; else globalThis.document = previous; });
  const output = document.createElement("pre"), display = new TerminalDisplay(output);
  for (const modify of [value => { value.display.styles[0].foreground.red=256; },value => { value.display.styles[0].underline="invalid"; },
    value => { value.display.rows[1]=[run("too wide",9)]; },value => { value.display.rows[1]=[run("bad",3,1)]; },
    value => { value.display.columns=401; }]) {
    assert.equal(display.render(frame()),true);
    const value = frame({text:"readable <fallback>"}); modify(value);
    assert.equal(display.render(value),false); assert.equal(output.textContent,"readable <fallback>");
    assert.equal(display.grid,null); assert.equal(output.children.length,0); assert.equal(display.text,"readable <fallback>");
    assert.equal(output.style.color,undefined); assert.equal(output.style.backgroundColor,undefined);
  }
  assert.equal(display.render(frame()),true); assert.equal(output.children.length,1);
});
