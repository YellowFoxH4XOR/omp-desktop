import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { z } from 'zod';
import type { AgentInfo, BackendEvent, ChangesSummary, GitFile, HarnessInstallation, HarnessKind, LoginProvider, ModelInfo, Project, RpcMessage, SessionSnapshot, SessionState, Thread, UiResponse, Usage } from './types';

export const api = {
  detectHarnesses: () => invoke<HarnessInstallation[]>('detect_harnesses'),
  setExecutableOverride: (kind: HarnessKind, path: string) => invoke<HarnessInstallation>('set_executable_override', { kind, path }),
  installHarness: (kind: HarnessKind) => invoke<void>('install_harness', { kind }),
  listProjects: () => invoke<Project[]>('list_projects'),
  addProject: (path: string, harness: HarnessKind) => invoke<Project>('add_project', { path, harness }),
  removeProject: (projectId: string) => invoke<void>('remove_project', { projectId }),
  listThreads: (projectId: string) => invoke<Thread[]>('list_threads', { projectId }),
  createThread: (projectId: string, harness: HarnessKind, isolated = false) => invoke<Thread>('create_thread', { projectId, harness, isolated }),
  openThread: (threadId: string) => invoke<SessionSnapshot>('open_thread', { threadId }),
  stopThread: (threadId: string) => invoke<void>('stop_thread', { threadId }),
  restartThread: (threadId: string) => invoke<SessionSnapshot>('restart_thread', { threadId }),
  sendPrompt: (threadId: string, message: string, mode: 'prompt' | 'steer' | 'follow_up' = 'prompt') => invoke<void>('send_prompt', { threadId, message, mode }),
  abortThread: (threadId: string) => invoke<void>('abort_thread', { threadId }),
  setThreadModel: (threadId: string, provider: string, modelId: string) => invoke<SessionState>('set_thread_model', { threadId, provider, modelId }),
  setThreadEffort: (threadId: string, level: string) => invoke<SessionState>('set_thread_effort', { threadId, level }),
  getModels: (threadId: string) => invoke<ModelInfo[]>('get_models', { threadId }),
  getEffortLevels: (threadId: string) => invoke<string[]>('get_effort_levels', { threadId }),
  getUsage: (threadId: string) => invoke<Usage>('get_usage', { threadId }),
  getLoginProviders: (threadId: string) => invoke<LoginProvider[]>('get_login_providers', { threadId }),
  loginProvider: (threadId: string, providerId: string) => invoke<void>('login_provider', { threadId, providerId }),
  renameThread: (threadId: string, title: string) => invoke<Thread>('rename_thread', { threadId, title }),
  setThreadFlags: (threadId: string, pinned?: boolean, archived?: boolean) => invoke<Thread>('set_thread_flags', { threadId, pinned, archived }),
  respondUi: (threadId: string, requestId: string, response: UiResponse) => invoke<void>('respond_ui', { threadId, requestId, response }),
  getSubagentMessages: (threadId: string, agentId: string) => invoke<RpcMessage[]>('get_subagent_messages', { threadId, agentId }),
  getSubagents: (threadId: string) => invoke<AgentInfo[]>('get_subagents', { threadId }),
  gitStatus: (threadId: string) => invoke<ChangesSummary>('git_status', { threadId }),
  gitFile: (threadId: string, path: string) => invoke<GitFile>('git_file', { threadId, path }),
  gitRevertFile: (threadId: string, path: string) => invoke<void>('git_revert_file', { threadId, path }),
  gitWriteIfUnchanged: (threadId: string, path: string, expectedHash: string, content: string) => invoke<void>('git_write_if_unchanged', { threadId, path, expectedHash, content }),
};

const eventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('rpc'), threadId: z.string(), frame: z.record(z.string(), z.unknown()) }),
  z.object({ type: z.literal('exited'), threadId: z.string(), code: z.number().nullish().transform(value => value ?? undefined), stderr: z.string(), expected: z.boolean() }),
  z.object({ type: z.literal('git_changed'), threadId: z.string() }),
  z.object({ type: z.literal('install_progress'), kind: z.enum(['omp', 'pi']), line: z.string() }),
  z.object({ type: z.literal('install_finished'), kind: z.enum(['omp', 'pi']), success: z.boolean(), error: z.string().nullish().transform(value => value ?? undefined) }),
]) satisfies z.ZodType<BackendEvent>;

/** RPC frames are untrusted. Validate the envelope before the session reducer sees it. */
export function onBackendEvent(handler: (event: BackendEvent) => void): Promise<UnlistenFn> {
  return listen<unknown>('desktop-event', ({ payload }) => {
    const parsed = eventSchema.safeParse(payload);
    if (parsed.success) handler(parsed.data);
  });
}
