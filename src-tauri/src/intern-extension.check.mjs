import test from 'node:test';
import assert from 'node:assert/strict';
import extension from './intern-extension.mjs';

function harness() {
  const hooks = new Map();
  let tool, active;
  extension({
    registerTool(value) { tool = value; },
    on(name, callback) { hooks.set(name, callback); },
    registerCommand() {},
    setActiveTools(value) { active = value; },
  });
  return { hooks, get tool() { return tool; }, get active() { return active; } };
}
test('the bridge adds pidesk without restricting Pi tools, extensions, or shell', () => {
  const pi = harness();
  assert.equal(pi.tool.name, 'pidesk');
  assert.equal(pi.active, undefined);
  for (const hook of ['session_start', 'tool_call', 'user_bash']) assert.equal(pi.hooks.has(hook), false);
});
test('plans are sent to the host; a refusal or cancellation never executes locally', async () => {
  const pi = harness();
  const request = {op:'plan', summary:'Write a file', actions:[{kind:'write_file',scope:'project',path:'test.txt',expected:null,content:'approved only'}]};
  let sent;
  const ctx = {mode:'rpc',hasUI:true,ui:{input:async (title,raw) => {sent={title,raw}; return JSON.stringify({ok:false,error:'User rejected'}); }}};
  await assert.rejects(() => pi.tool.execute('one',request,new AbortController().signal,undefined,ctx), /User rejected/);
  assert.equal(sent.title,'PIDESK_INTERN_V1');
  assert.deepEqual(JSON.parse(sent.raw),request);
  ctx.ui.input=async()=>undefined;
  await assert.rejects(() => pi.tool.execute('two',request,undefined,undefined,ctx), /cancelled/);
  await assert.rejects(() => pi.tool.execute('three',request,undefined,undefined,{mode:'tui',hasUI:true}), /requires/);
});
test('Intern orchestrates full-Pi project threads instead of guarded workers', () => {
  const pi = harness();
  assert.match(pi.tool.description, /create_thread.*full tools/);
  assert.doesNotMatch(pi.tool.description, /adopt_thread|guarded/);
});
