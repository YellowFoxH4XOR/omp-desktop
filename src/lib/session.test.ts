import { expect, test } from 'vitest';
import { SessionModel } from './session.svelte';
import type { RpcMessage, SessionSnapshot } from './types';

const snapshot: SessionSnapshot = {
  thread: {
    id: 'thread-1', projectId: 'project-1', harness: 'omp', sessionId: 'session-1',
    sessionFile: '/tmp/session.jsonl', cwd: '/tmp/project', title: 'Fixture',
    pinned: false, archived: false, status: 'idle',
    createdAt: '2026-09-23T00:00:00Z', lastViewedAt: '2026-09-23T00:00:00Z'
  },
  messages: [],
  state: { sessionId: 'session-1', isStreaming: false },
  models: [], levels: [], agents: [],
  capabilities: {
    agents: true, nestedAgents: true, agentSteering: false, agentKill: false,
    agentRevive: false, planMode: false, permissions: true, modelSwitching: true,
    effortLevels: true, contextUsage: true, tokenUsage: true, worktrees: true
  }
};

function assistantTexts(model: SessionModel): string[] {
  return model.view.items.flatMap(item => item.kind === 'text' ? [item.text] : []);
}

test('OMP full message_start followed by deltas renders the answer once', () => {
  const model = new SessionModel(snapshot);
  const answer: RpcMessage = {
    role: 'assistant', content: [{ type: 'text', text: 'One streamed answer.' }],
    timestamp: 1, stopReason: 'stop'
  };
  model.apply({ type: 'agent_start' });
  model.apply({ type: 'message_start', message: answer });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'text_start', contentIndex: 0 } });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'One streamed answer.' } });
  expect(assistantTexts(model)).toEqual(['One streamed answer.']);
  model.apply({ type: 'message_end', message: answer });
  model.apply({ type: 'agent_end', messages: [answer], isTerminal: true });
  expect(assistantTexts(model)).toEqual(['One streamed answer.']);
  expect(model.view.status).toBe('completed');
});

test('OMP approval select remains an explicit permission request', () => {
  const model = new SessionModel(snapshot);
  model.apply({ type: 'extension_ui_request', id: 'approval-1', method: 'select', title: 'Allow tool: bash\nCommand: rm -rf build/', options: ['Approve', 'Deny'] });
  expect(model.view.pendingRequests).toMatchObject([{ id: 'approval-1', method: 'permission', toolName: 'bash' }]);
  expect(model.view.status).toBe('waiting');
  model.dismissRequest('approval-1');
  expect(model.view.pendingRequests).toHaveLength(0);
});

test('restart preserves an unsaved streaming tail and reconciles durable history', () => {
  const user: RpcMessage = { role: 'user', content: 'Investigate this', timestamp: 1 };
  const model = new SessionModel({ ...snapshot, messages: [user] });
  model.apply({ type: 'agent_start' });
  model.apply({ type: 'message_start', message: { role: 'assistant', content: [], timestamp: 2 } });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'Unpersisted thought' } });
  model.setError('Harness stopped unexpectedly');
  expect(model.view.items.some(item => item.kind === 'text' && item.text === 'Unpersisted thought')).toBe(true);

  const durable: RpcMessage = { role: 'assistant', content: [{ type: 'text', text: 'Saved answer' }], timestamp: 3 };
  model.reconnect({ ...snapshot, messages: [user, durable] });
  expect(model.view.error).toBeUndefined();
  expect(model.view.items.filter(item => item.kind === 'user')).toHaveLength(1);
  expect(assistantTexts(model)).toEqual(['Unpersisted thought', 'Saved answer']);
  model.apply({ type: 'agent_start' });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'New reply' } });
  expect(assistantTexts(model)).toEqual(['Unpersisted thought', 'Saved answer', 'New reply']);
});

test('restart replaces an interrupted partial with its completed persisted text', () => {
  const model = new SessionModel(snapshot);
  model.apply({ type: 'agent_start' });
  model.apply({ type: 'message_start', message: { role: 'assistant', content: [], timestamp: 42 } });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'Half' } });
  model.setError('Disconnected');
  const completed: RpcMessage = { role: 'assistant', content: [{ type: 'text', text: 'Half and complete' }], timestamp: 42 };
  model.reconnect({ ...snapshot, messages: [completed] });
  expect(assistantTexts(model)).toEqual(['Half and complete']);
});

test('opening a busy thread preserves its prior completed assistant turn', () => {
  const previous: RpcMessage = {
    role: 'assistant', content: [{ type: 'text', text: 'Previous answer' }],
    timestamp: 1, stopReason: 'stop'
  };
  const next: RpcMessage = {
    role: 'assistant', content: [{ type: 'text', text: 'Current answer' }],
    timestamp: 2, stopReason: 'stop'
  };
  const model = new SessionModel({
    ...snapshot,
    messages: [previous],
    state: { sessionId: 'session-1', isStreaming: true }
  });
  model.apply({ type: 'message_start', message: { role: 'assistant', content: [], timestamp: 2 } });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', contentIndex: 0, delta: 'Current answer' } });
  model.apply({ type: 'message_end', message: next });
  expect(assistantTexts(model)).toEqual(['Previous answer', 'Current answer']);
});

test('timestamp-less equal-length assistant messages do not collide', () => {
  const model = new SessionModel(snapshot);
  const left: RpcMessage = { role: 'assistant', content: [{ type: 'text', text: 'Left answer' }] };
  const right: RpcMessage = { role: 'assistant', content: [{ type: 'text', text: 'Other reply' }] };
  model.apply({ type: 'message_end', message: left });
  model.apply({ type: 'message_end', message: right });
  expect(assistantTexts(model)).toEqual(['Left answer', 'Other reply']);
});

test('exact timestamp-less message replays are suppressed', () => {
  const model = new SessionModel(snapshot);
  const message: RpcMessage = { role: 'assistant', content: [{ type: 'text', text: 'Only once' }] };
  model.apply({ type: 'message_end', message });
  model.apply({ type: 'message_end', message });
  expect(assistantTexts(model)).toEqual(['Only once']);
});

test('turn_end does not duplicate a message_end that only gained usage metadata', () => {
  const model = new SessionModel(snapshot);
  const delivered: RpcMessage = {
    role: 'assistant',
    content: [{ type: 'thinking', thinking: 'Need no tools.' }, { type: 'text', text: '4' }],
    timestamp: 42,
    stopReason: 'stop'
  };
  const repeated = {
    ...delivered,
    usage: { input: 10, output: 1, total: 11 }
  } as RpcMessage;
  model.apply({ type: 'agent_start' });
  model.apply({ type: 'message_end', message: delivered });
  model.apply({ type: 'turn_end', message: repeated });
  expect(model.view.items.filter(item => item.kind === 'thinking')).toHaveLength(1);
  expect(assistantTexts(model)).toEqual(['4']);
});

test('stable message IDs distinguish identical payloads', () => {
  const model = new SessionModel(snapshot);
  const first = { role: 'assistant', id: 'message-1', content: [{ type: 'text', text: 'Same payload' }] } as RpcMessage;
  const second = { role: 'assistant', id: 'message-2', content: [{ type: 'text', text: 'Same payload' }] } as RpcMessage;
  model.apply({ type: 'message_end', message: first });
  model.apply({ type: 'message_end', message: second });
  expect(assistantTexts(model)).toEqual(['Same payload', 'Same payload']);
});

test('a large terminal agent-end summary does not duplicate delivered messages', () => {
  const model = new SessionModel(snapshot);
  const messages: RpcMessage[] = Array.from({ length: 2050 }, (_, index) => ({
    role: 'user', content: `Message ${index}`, timestamp: index + 1
  }));
  model.apply({ type: 'agent_start' });
  for (const message of messages) model.apply({ type: 'message_end', message });
  model.apply({ type: 'agent_end', messages, isTerminal: true });
  expect(model.view.items.filter(item => item.kind === 'user')).toHaveLength(messages.length);
});

test('long sessions preserve replayed messages, tool ordering, nested agents, and status within budget', () => {
  const messages: RpcMessage[] = Array.from({ length: 5000 }, (_, index) => ({
    role: 'user', content: `Message ${index}`, timestamp: index + 1
  }));
  const model = new SessionModel({ ...snapshot, messages });
  model.apply({ type: 'agent_start' });
  for (const message of messages.slice(-1500)) model.apply({ type: 'turn_end', message });
  for (let index = 0; index < 100; index++) {
    const toolCallId = `tool-${index}`;
    model.apply({ type: 'tool_execution_start', toolCallId, toolName: 'stress', intent: `Step ${index}` });
    model.apply({
      type: 'tool_execution_end', toolCallId,
      result: { content: [{ type: 'text', text: `Result ${index}` }] }
    });
  }
  const agentIds = ['Stress-0'];
  for (let index = 1; index < 10; index++) agentIds.push(`${agentIds.at(-1)}.Level-${index}`);
  for (const id of agentIds) {
    model.apply({ type: 'subagent_lifecycle', payload: { id, agent: 'task', status: 'started' } });
  }
  for (const id of agentIds) {
    model.apply({ type: 'subagent_lifecycle', payload: { id, agent: 'task', status: 'completed' } });
  }
  model.apply({ type: 'agent_settled' });

  const users = model.view.items.filter(item => item.kind === 'user');
  const tools = model.view.items.filter(item => item.kind === 'tool');
  expect(users).toHaveLength(5000);
  expect(users[0]).toMatchObject({ text: 'Message 0', timestamp: 1 });
  expect(users.at(-1)).toMatchObject({ text: 'Message 4999', timestamp: 5000 });
  expect(tools).toHaveLength(100);
  expect(tools.map(tool => tool.kind === 'tool' ? tool.toolCallId : '')).toEqual(
    Array.from({ length: 100 }, (_, index) => `tool-${index}`)
  );
  expect(tools.every(tool => tool.kind === 'tool' && tool.status === 'completed')).toBe(true);
  expect(model.view.agents.map(agent => agent.name)).toEqual(agentIds.map(id => id.split('.').at(-1)));
  expect(model.view.agents.every(agent => agent.status === 'completed')).toBe(true);
  expect(model.view.status).toBe('completed');
});

test('late same-id prompt failure ends the run with an actionable error', () => {
  const model = new SessionModel(snapshot);
  model.apply({ type: 'agent_start' });
  model.apply({ type: 'response', command: 'prompt', success: false, error: 'Model could not start' });
  expect(model.view.status).toBe('failed');
  expect(model.view.items.some(item => item.kind === 'notice' && item.text.includes('Model could not start'))).toBe(true);
});

test('nested worker progress preserves names, hierarchy, activity, and model effort', () => {
  const model = new SessionModel(snapshot);
  model.apply({ type: 'subagent_lifecycle', payload: {
    id: 'Backend', agent: 'task', agentSource: 'bundled', status: 'started', index: 0
  } });
  model.apply({ type: 'subagent_lifecycle', payload: {
    id: 'Backend.DatabaseExpert', agent: 'scout', agentSource: 'bundled',
    parentToolCallId: 'call-1', status: 'started', index: 0
  } });
  model.apply({ type: 'subagent_progress', payload: {
    agent: 'scout', agentSource: 'bundled', parentToolCallId: 'call-1',
    progress: { id: 'Backend.DatabaseExpert', status: 'running', lastIntent: 'Reading schema.rs',
      resolvedModelIdentity: 'openai/gpt-small', resolvedThinkingLevel: 'high', tokens: 2048 }
  } });
  expect(model.view.agents.map(agent => agent.name)).toEqual(['Backend', 'DatabaseExpert']);
  expect(model.view.agents[1]).toMatchObject({
    parentId: 'Backend', parentToolCallId: 'call-1', role: 'scout',
    activity: 'Reading schema.rs', model: 'openai/gpt-small', effort: 'high', tokens: 2048
  });
  model.apply({ type: 'subagent_lifecycle', payload: {
    id: 'Backend.DatabaseExpert', agent: 'scout', status: 'completed', index: 0
  } });
  expect(model.view.agents[1].status).toBe('completed');
});

test('live bash output is capped to a tail and still reconciles with the final message', () => {
  const model = new SessionModel(snapshot);
  const chunk = 'x'.repeat(16 * 1024);
  for (let i = 0; i < 16; i++) model.apply({ type: 'bash_execution_update', id: 'b1', delta: chunk });
  model.apply({ type: 'bash_execution_update', id: 'b1', delta: 'END' });
  const live = model.view.items.filter(item => item.kind === 'custom' && item.customType === 'bashExecution');
  expect(live).toHaveLength(1);
  expect(live[0].kind === 'custom' && live[0].text.length).toBeLessThanOrEqual(64 * 1024);
  expect(live[0].kind === 'custom' && live[0].text.endsWith('END')).toBe(true);

  model.apply({ type: 'message_end', message: { role: 'bashExecution', command: 'yes', output: `${chunk.repeat(16)}END`, exitCode: 0, timestamp: 5 } });
  const settled = model.view.items.filter(item => item.kind === 'custom' && item.customType === 'bashExecution');
  expect(settled).toHaveLength(1);
  expect(settled[0].kind === 'custom' && settled[0].text).toBe('yes');
});

test('tool call argument deltas do not create duplicate rows', () => {
  const model = new SessionModel(snapshot);
  model.apply({ type: 'agent_start' });
  model.apply({ type: 'message_start', message: { role: 'assistant', content: [], timestamp: 9 } });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'toolcall_start', contentIndex: 0, id: 'call-1', name: 'write' } });
  for (let i = 0; i < 50; i++) model.apply({ type: 'message_update', assistantMessageEvent: { type: 'toolcall_delta', contentIndex: 0, delta: '{"content":"aaaa' } });
  model.apply({ type: 'message_update', assistantMessageEvent: { type: 'toolcall_end', contentIndex: 0, toolCall: { id: 'call-1', name: 'write', arguments: { path: 'a.txt' } } } });
  const tools = model.view.items.filter(item => item.kind === 'tool');
  expect(tools).toHaveLength(1);
  expect(tools[0].kind === 'tool' && tools[0].args).toEqual({ path: 'a.txt' });
});
