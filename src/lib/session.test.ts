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
