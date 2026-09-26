import { expect, test, vi } from 'vitest';
import { onBackendEvent } from './api';
import type { BackendEvent } from './types';

const bridge = vi.hoisted(() => ({ receive: (_event: { payload: unknown }) => {} }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_channel: string, receive: typeof bridge.receive) => {
    bridge.receive = receive;
    return () => {};
  }),
}));

test('install events accept Pi and reject unsupported harnesses', async () => {
  const events: BackendEvent[] = [];
  await onBackendEvent(event => events.push(event));
  for (const kind of ['omp', 'unknown', 'pi']) {
    bridge.receive({ payload: { type: 'install_progress', kind, line: 'Installing' } });
    bridge.receive({ payload: { type: 'install_finished', kind, success: true } });
  }
  expect(events).toEqual([
    { type: 'install_progress', kind: 'pi', line: 'Installing' },
    { type: 'install_finished', kind: 'pi', success: true },
  ]);
});

test('installer stages are validated and oversized log lines are rejected', async () => {
  const events: BackendEvent[] = [];
  await onBackendEvent(event => events.push(event));
  for (const stage of ['preparing', 'installing', 'verifying', 'unknown']) {
    bridge.receive({ payload: { type: 'install_stage', kind: 'pi', stage } });
  }
  bridge.receive({ payload: { type: 'install_stage', kind: 'omp', stage: 'installing' } });
  bridge.receive({ payload: { type: 'install_progress', kind: 'pi', line: 'x'.repeat(16 * 1024 + 1) } });
  expect(events).toEqual(['preparing', 'installing', 'verifying'].map(stage => ({ type: 'install_stage', kind: 'pi', stage })));
});

test('malformed RPC envelopes never reach the session reducer', async () => {
  const receive = vi.fn();
  await onBackendEvent(receive);
  bridge.receive({ payload: { type: 'rpc', threadId: 'thread', frame: [] } });
  bridge.receive({ payload: { type: 'rpc', threadId: 42, frame: {} } });
  expect(receive).not.toHaveBeenCalled();
});
