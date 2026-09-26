import test from 'node:test';
import assert from 'node:assert/strict';
import { register } from 'node:module';

// Pi supplies @earendil-works/pi-coding-agent to extensions; stub it here.
register('data:text/javascript,' + encodeURIComponent(`
  export async function resolve(specifier, context, next) {
    if (specifier === '@earendil-works/pi-coding-agent') return { url: 'data:text/javascript,export function createBashTool() { return { name: "bash", execute: async () => ({}) } }', shortCircuit: true };
    return next(specifier, context);
  }`));
const { default: extension, describeCall, foreignPath, isMcpLookup, isReadOnlyCommand, realHomeBash, realHomeEnv } = await import('./pidesk-modes.mjs');

function harness(flags = {}) {
  const hooks = new Map();
  const commands = new Map();
  const statuses = [];
  let tool;
  extension({
    registerFlag() {},
    getFlag(name) { return flags[name]; },
    registerCommand(name, value) { commands.set(name, value); },
    registerTool(value) { if (value.name !== 'bash') tool = value; },
    on(name, callback) { hooks.set(name, callback); },
  });
  const asked = [];
  const ctx = { hasUI: true, ui: { setStatus: (_key, value) => statuses.push(value), select: async () => undefined } };
  hooks.get('session_start')({}, ctx);
  // `answer` is what the user picks if the call needs permission.
  const blocked = async (toolName, input = {}, answer = 'Keep blocked') => {
    const ask = { ...ctx, ui: { ...ctx.ui, select: async (title, options) => { asked.push({ title, options }); return answer; } } };
    return Boolean((await hooks.get('tool_call')({ toolName, input }, ask))?.block);
  };
  return { hooks, commands, statuses, ctx, blocked, asked, get tool() { return tool; } };
}

test("the agent's shell gets the real home while Pi keeps the private one", async () => {
  const env = { HOME: '/Users/me/.pidesk/home', XDG_CONFIG_HOME: '/Users/me/.pidesk/home/.config', XDG_CACHE_HOME: 'x', PIDESK_REAL_HOME: '/Users/me', PATH: '/bin', PI_SESSION_ID: 's1' };
  assert.deepEqual(realHomeEnv(env, '/Users/me'), { HOME: '/Users/me', PATH: '/bin', PI_SESSION_ID: 's1' });
  let options;
  const tool = realHomeBash((cwd, opts) => { options = { cwd, ...opts }; return { name: 'bash', execute: async (id, params) => ({ id, params }) }; }, '/repo', '/Users/me');
  assert.equal(tool.name, 'bash');
  assert.equal(options.cwd, '/repo');
  const spawned = options.spawnHook({ command: 'git log', cwd: '/repo', env });
  assert.equal(spawned.command, 'git log');
  assert.equal(spawned.env.HOME, '/Users/me');
  assert.equal(spawned.env.XDG_CONFIG_HOME, undefined);
  assert.deepEqual(await tool.execute('1', { command: 'x' }), { id: '1', params: { command: 'x' } });
});

test('read-only commands pass; writes, chaining, substitution and redirection do not', () => {
  for (const command of ['rg foo src', 'git status', 'git diff HEAD~1 -- a.ts', 'cat a | grep b | wc -l', 'ls -la && git log --oneline -5', 'sed -n 1,20p x', 'curl -s https://example.com', 'cd /tmp/p && git branch -a', 'rg foo 2>/dev/null', 'git log 2>&1 | head'])
    assert.equal(isReadOnlyCommand(command), true, command);
  for (const command of ['rm -rf x', 'cat a > b', 'echo hi >> f', 'cat a; python evil.py', 'ls && npm install', 'echo $(rm x)', 'find . -delete', 'find . -exec rm {} ;', 'sed -i s/a/b/ f', 'git checkout .', 'git branch newname', 'git remote add o u', 'curl -o f https://x', 'curl -X POST https://x', 'env rm x', 'awk "BEGIN{system(1)}"', 'npm test', 'ls & rm x', 'cat x 2>out.txt', 'ls >/dev/null; rm x', 'cat a 1>&2 > f', ''])
    assert.equal(isReadOnlyCommand(command), false, command);
});

test('Auto is the default and blocks nothing', async () => {
  const pi = harness();
  assert.equal(pi.statuses.at(-1), 'auto');
  for (const toolName of ['edit', 'write', 'bash', 'custom_tool']) assert.equal(await pi.blocked(toolName, { command: 'rm -rf x' }), false);
  assert.equal(pi.asked.length, 0);
  assert.equal(pi.hooks.get('before_agent_start')(), undefined);
});

test('Plan lets reads through and asks before anything else', async () => {
  const pi = harness({ 'pidesk-plan': true });
  assert.equal(pi.statuses.at(-1), 'plan');
  for (const toolName of ['read', 'grep', 'find', 'ls', 'pidesk', 'request_auto']) assert.equal(await pi.blocked(toolName), false, toolName);
  assert.equal(await pi.blocked('bash', { command: 'git diff' }), false);
  // MCP metadata lookups are reads; calling or connecting is not.
  for (const input of [{}, { server: 'github' }, { search: 'screenshot', limit: 5 }, { describe: 'x_y' }, { instructions: 'github' }]) assert.equal(await pi.blocked('mcp', input), false, JSON.stringify(input));
  assert.equal(pi.asked.length, 0);
  for (const toolName of ['edit', 'write', 'custom_tool']) assert.equal(await pi.blocked(toolName), true, toolName);
  assert.equal(await pi.blocked('bash', { command: 'npm install' }), true);
  for (const input of [{ tool: 'github_create_issue', args: {} }, { connect: 'github' }, { action: 'auth-start', server: 'x' }]) assert.equal(await pi.blocked('mcp', input), true, JSON.stringify(input));
  assert.equal(pi.asked.length, 7);
  assert.match(pi.asked[3].title, /^Plan mode: allow this\?\n\nRun in the shell:\nnpm install$/);
  assert.deepEqual(pi.asked[0].options, ['Allow once', 'Allow for this run (Auto)', 'Keep blocked']);
  assert.equal(isMcpLookup({ search: 'a', tool: '' }), true);
  assert.throws(() => pi.hooks.get('user_bash')(), /read-only/);
  assert.equal(pi.hooks.get('before_agent_start')().message.display, false);
});

test('allow once runs one call; allow for this run turns on Auto until it ends', async () => {
  const pi = harness({ 'pidesk-plan': true });
  assert.equal(await pi.blocked('bash', { command: 'npm test' }, 'Allow once'), false);
  assert.equal(await pi.blocked('bash', { command: 'npm test' }), true);
  assert.equal(await pi.blocked('edit', { path: 'a.ts' }, 'Allow for this run (Auto)'), false);
  assert.equal(pi.statuses.at(-1), 'plan:approved');
  const asked = pi.asked.length;
  assert.equal(await pi.blocked('write', { path: 'b.ts', content: 'x' }), false);
  assert.equal(pi.asked.length, asked);
  pi.hooks.get('agent_end')({}, pi.ctx);
  assert.equal(await pi.blocked('write', { path: 'b.ts', content: 'x' }), true);
});

test('with no πDesk window to ask in, Plan still blocks', async () => {
  const pi = harness({ 'pidesk-plan': true });
  const result = await pi.hooks.get('tool_call')({ toolName: 'edit', input: { path: 'a.ts' } }, { hasUI: false });
  assert.equal(result.block, true);
});

test("reaching into another app's config asks in either mode, once per session if allowed", async () => {
  for (const [tool, input] of [
    ['bash', { command: 'cat ~/.config/mcp/mcp.json' }],
    ['bash', { command: 'ls -a $HOME/.pi/agent' }],
    ['bash', { command: 'head /Users/me/.claude.json' }],
    ['read', { path: '/Users/me/.pi/agent/mcp.json' }],
    ['grep', { pattern: 'token', path: '/Users/me/.aws' }],
  ]) assert.ok(foreignPath(tool, input, '/Users/me'), JSON.stringify(input));
  for (const [tool, input] of [
    ['bash', { command: 'cat ~/.pidesk/agent/settings.json' }],
    ['bash', { command: 'ls ~/.pinned-notes ~/.config/mcpx' }],
    ['read', { path: 'src/.pi/notes.md' }],
    ['edit', { path: '/Users/me/.pi/agent/mcp.json' }],
  ]) assert.equal(foreignPath(tool, input, '/Users/me'), null, JSON.stringify(input));

  process.env.PIDESK_REAL_HOME = '/Users/me';
  try {
    const pi = harness();
    assert.equal(await pi.blocked('bash', { command: 'cat ~/.config/mcp/mcp.json' }), true);
    assert.match(pi.asked[0].title, /^Read another app's files\?\n\nRun in the shell:\ncat ~\/\.config\/mcp\/mcp\.json\n\n~\/\.config\/mcp belongs to another app/);
    assert.deepEqual(pi.asked[0].options, ['Allow once', 'Allow for this session', 'Keep blocked']);
    assert.equal(await pi.blocked('bash', { command: 'cat ~/.config/mcp/mcp.json' }, 'Allow once'), false);
    assert.equal(await pi.blocked('read', { path: '/Users/me/.config/mcp/mcp.json' }, 'Allow for this session'), false);
    const asked = pi.asked.length;
    assert.equal(await pi.blocked('bash', { command: 'cat ~/.config/mcp/other.json' }), false);
    assert.equal(pi.asked.length, asked);
    // A different app's folder still asks.
    assert.equal(await pi.blocked('bash', { command: 'ls ~/.pi' }), true);
    // Allowed reads still follow Plan rules afterwards.
    const planned = harness({ 'pidesk-plan': true });
    assert.equal(await planned.blocked('bash', { command: 'cat ~/.claude.json' }, 'Allow once'), false);
  } finally {
    delete process.env.PIDESK_REAL_HOME;
  }
});

test('permission cards say exactly what would happen', () => {
  assert.equal(describeCall('read', { path: '/x/mcp.json' }), 'Read /x/mcp.json');
  assert.equal(describeCall('grep', { path: 'src', pattern: 'token' }), 'Search src for “token”');
  assert.equal(describeCall('edit', { path: 'src/a.ts', edits: [{}, {}] }), 'Edit src/a.ts (2 changes)');
  assert.equal(describeCall('write', { path: 'b.md', content: 'hello' }), 'Write b.md (5 characters)');
  assert.equal(describeCall('mcp', { tool: 'github_create_issue', args: { title: 'x' } }), 'Use MCP: github_create_issue\n{"title":"x"}');
  assert.equal(describeCall('custom', {}), 'Use custom');
  assert.ok(describeCall('bash', { command: 'x'.repeat(5000) }).length < 1300);
});

test('approval enables Auto for the rest of the run, then Plan returns', async () => {
  const pi = harness({ 'pidesk-plan': true });
  let title;
  let options;
  pi.ctx.ui.select = async (value, choices) => { title = value; options = choices; return choices[2]; };
  const declined = await pi.tool.execute('1', { plan: '1. Edit a.ts' }, undefined, undefined, pi.ctx);
  assert.deepEqual(options, ['Approve and run in Auto', 'Revise with feedback', 'Decline']);
  assert.match(declined.content[0].text, /declined/);
  assert.equal(await pi.blocked('edit'), true);
  pi.ctx.ui.select = async (_value, choices) => choices[1];
  const revise = await pi.tool.execute('1b', { plan: '1. Edit a.ts' }, undefined, undefined, pi.ctx);
  assert.match(revise.content[0].text, /feedback arrives as the next message/);
  assert.equal(await pi.blocked('edit'), true);
  pi.ctx.ui.select = async (value, options) => { title = value; return options[0]; };
  const approved = await pi.tool.execute('2', { plan: '1. Edit a.ts' }, undefined, undefined, pi.ctx);
  assert.match(title, /1\. Edit a\.ts/);
  assert.match(approved.content[0].text, /Approved/);
  assert.equal(pi.statuses.at(-1), 'plan:approved');
  assert.equal(await pi.blocked('edit'), false);
  assert.equal(await pi.blocked('bash', { command: 'npm test' }), false);
  pi.hooks.get('agent_end')({}, pi.ctx);
  assert.equal(pi.statuses.at(-1), 'plan');
  assert.equal(await pi.blocked('edit'), true);
});

test('host command switches modes, but a locked Plan never goes Auto by command', async () => {
  const open = harness();
  await open.commands.get('pidesk-mode').handler('plan', open.ctx);
  assert.equal(await open.blocked('write'), true);
  await open.commands.get('pidesk-mode').handler('auto', open.ctx);
  assert.equal(await open.blocked('write'), false);
  const locked = harness({ 'pidesk-plan-locked': true });
  await locked.commands.get('pidesk-mode').handler('auto', locked.ctx);
  assert.equal(await locked.blocked('write'), true);
});

test('context keeps only the newest Plan reminder, and none in Auto', async () => {
  const pi = harness({ 'pidesk-plan': true });
  const reminder = { customType: 'pidesk-plan-context' };
  const messages = [reminder, { role: 'user' }, { ...reminder }, { role: 'assistant' }];
  assert.deepEqual(pi.hooks.get('context')({ messages }).messages, [{ role: 'user' }, reminder, { role: 'assistant' }]);
  await pi.commands.get('pidesk-mode').handler('auto', pi.ctx);
  assert.deepEqual(pi.hooks.get('context')({ messages }).messages, [{ role: 'user' }, { role: 'assistant' }]);
});
