import { invoke, type Channel } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { z } from 'zod';
import type { BackendEvent, CatalogPage, ChangesSummary, InstalledPackage, McpOverview, GitFile, HarnessInstallation, HarnessInstallCommand, HarnessKind, ModelInfo, Project, SessionSnapshot, SessionState, Thread, ThreadDeletePreview, ThreadMode, UiResponse, Usage, RuntimeStats, ModelDefaults, InternImage, InternPlan } from './types';

export const api = {
  internSnapshot: () => invoke<SessionSnapshot>('intern_snapshot'),
  internPrompt: (message: string, projectId: string | undefined, images: InternImage[]) => invoke<void>('intern_prompt', { message, projectId, images }),
  internPlans: () => invoke<InternPlan[]>('intern_plans'),
  internApprove: (planId: string, approved: boolean) => invoke<void>('intern_approve', { planId, approved }),
  internStop: () => invoke<void>('intern_stop'),
  internClear: () => invoke<SessionSnapshot>('intern_clear'),
  detectHarnesses: () => invoke<HarnessInstallation[]>('detect_harnesses'),
  installHarness: (kind: HarnessKind) => invoke<void>('install_harness', { kind }),
  harnessInstallCommands: () => invoke<HarnessInstallCommand[]>('harness_install_commands'),
  listProjects: () => invoke<Project[]>('list_projects'),
  addProject: (path: string, harness: HarnessKind) => invoke<Project>('add_project', { path, harness }),
  removeProject: (projectId: string) => invoke<void>('remove_project', { projectId }),
  listThreads: (projectId: string) => invoke<Thread[]>('list_threads', { projectId }),
  listRecentThreads: (limit: number) => invoke<Thread[]>('list_recent_threads', { limit }),
  createThread: (projectId: string, harness: HarnessKind, isolated = false) => invoke<Thread>('create_thread', { projectId, harness, isolated }),
  openThread: (threadId: string) => invoke<SessionSnapshot>('open_thread', { threadId }),
  stopThread: (threadId: string) => invoke<void>('stop_thread', { threadId }),
  threadDeletePreview: (threadId: string) => invoke<ThreadDeletePreview>('thread_delete_preview', { threadId }),
  deleteThread: (threadId: string, discardChanges: boolean) => invoke<void>('delete_thread', { threadId, discardChanges }),
  getRuntimeStats: () => invoke<RuntimeStats>('get_runtime_stats'),
  getModelDefaults: () => invoke<ModelDefaults>('get_model_defaults'),
  setDefaultModel: (provider: string, modelId: string) => invoke<ModelDefaults>('set_default_model', { provider, modelId }),
  setDefaultThinkingLevel: (level: string) => invoke<ModelDefaults>('set_default_thinking_level', { level }),
  prewarmThread: (threadId: string) => invoke<void>('prewarm_thread', { threadId }),
  restartThread: (threadId: string) => invoke<SessionSnapshot>('restart_thread', { threadId }),
  sendPrompt: (threadId: string, message: string, mode: 'prompt' | 'steer' | 'follow_up' = 'prompt') => invoke<void>('send_prompt', { threadId, message, mode }),
  abortThread: (threadId: string) => invoke<void>('abort_thread', { threadId }),
  setThreadModel: (threadId: string, provider: string, modelId: string) => invoke<SessionState>('set_thread_model', { threadId, provider, modelId }),
  setThreadEffort: (threadId: string, level: string) => invoke<SessionState>('set_thread_effort', { threadId, level }),
  getModels: (threadId: string) => invoke<ModelInfo[]>('get_models', { threadId }),
  getEffortLevels: (threadId: string) => invoke<string[]>('get_effort_levels', { threadId }),
  getUsage: (threadId: string) => invoke<Usage>('get_usage', { threadId }),
  compactThread: (threadId: string, instructions?: string) => invoke<void>('compact_thread', { threadId, instructions }),
  listThreadFiles: (threadId: string) => invoke<{ files: string[]; truncated: boolean }>('list_thread_files', { threadId }),
  renameThread: (threadId: string, title: string) => invoke<Thread>('rename_thread', { threadId, title }),
  setThreadMode: (threadId: string, mode: ThreadMode) => invoke<Thread>('set_thread_mode', { threadId, mode }),
  extensionsCatalog: (query: string, kind: string, sort: string, page: number) => invoke<CatalogPage>('extensions_catalog', { query, kind, sort, page }),
  extensionsInstalled: () => invoke<InstalledPackage[]>('extensions_installed'),
  extensionsCheckUpdates: () => invoke<InstalledPackage[]>('extensions_check_updates'),
  extensionsInstall: (source: string) => invoke<string>('extensions_install', { source }),
  extensionsUpdate: (source: string) => invoke<void>('extensions_update', { source }),
  extensionsRemove: (source: string) => invoke<void>('extensions_remove', { source }),
  mcpOverview: () => invoke<McpOverview>('mcp_overview'),
  mcpSaveServer: (original: string | null, name: string, config: Record<string, unknown>) => invoke<void>('mcp_save_server', { original, name, config }),
  mcpRemoveServer: (name: string) => invoke<void>('mcp_remove_server', { name }),
  mcpSetEnabled: (name: string, enabled: boolean) => invoke<void>('mcp_set_enabled', { name, enabled }),
  mcpSetApproveTools: (all: boolean) => invoke<void>('mcp_set_approve_tools', { all }),
  mcpSaveRaw: (text: string) => invoke<void>('mcp_save_raw', { text }),
  mcpImport: (source: string, names: string[]) => invoke<string[]>('mcp_import', { source, names }),
  mcpInstallAdapter: () => invoke<void>('mcp_install_adapter'),
  setThreadFlags: (threadId: string, pinned?: boolean, archived?: boolean) => invoke<Thread>('set_thread_flags', { threadId, pinned, archived }),
  respondUi: (threadId: string, requestId: string, response: UiResponse) => invoke<void>('respond_ui', { threadId, requestId, response }),
  terminalOpen: (cols: number, rows: number, cwd: string | undefined, output: Channel<ArrayBuffer>) => invoke<string>('terminal_open', { cols, rows, cwd, output }),
  terminalWrite: (id: string, data: string) => invoke<void>('terminal_write', { id, data }),
  terminalAck: (id: string, bytes: number) => invoke<void>('terminal_ack', { id, bytes }),
  terminalResize: (id: string, cols: number, rows: number) => invoke<void>('terminal_resize', { id, cols, rows }),
  terminalClose: (id: string) => invoke<void>('terminal_close', { id }),
  gitStatus: (threadId: string) => invoke<ChangesSummary>('git_status', { threadId }),
  gitFile: (threadId: string, path: string) => invoke<GitFile>('git_file', { threadId, path }),
  gitRevertFile: (threadId: string, path: string, expectedHash?: string | null) => invoke<void>('git_revert_file', { threadId, path, expectedHash: expectedHash ?? null }),
  gitWriteIfUnchanged: (threadId: string, path: string, expectedHash: string, content: string) => invoke<void>('git_write_if_unchanged', { threadId, path, expectedHash, content }),
  openChangedFile: (threadId: string, path: string) => invoke<void>('open_changed_file', { threadId, path }),
};

const eventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('intern_changed') }),
  z.object({ type: z.literal('rpc'), threadId: z.string(), frame: z.record(z.string(), z.unknown()) }),
  z.object({ type: z.literal('exited'), threadId: z.string(), code: z.number().nullish().transform(value => value ?? undefined), stderr: z.string(), expected: z.boolean() }),
  z.object({ type: z.literal('git_changed'), threadId: z.string() }),
  z.object({ type: z.literal('install_stage'), kind: z.literal('pi'), stage: z.enum(['preparing', 'installing', 'verifying']) }),
  z.object({ type: z.literal('install_progress'), kind: z.literal('pi'), line: z.string().max(16 * 1024) }),
  z.object({ type: z.literal('install_finished'), kind: z.literal('pi'), success: z.boolean(), error: z.string().nullish().transform(value => value ?? undefined) }),
]) satisfies z.ZodType<BackendEvent>;

/** RPC frames are untrusted. Validate the envelope before the session reducer sees it. */
export function onBackendEvent(handler: (event: BackendEvent) => void): Promise<UnlistenFn> {
  return listen<unknown>('desktop-event', ({ payload }) => {
    const parsed = eventSchema.safeParse(payload);
    if (parsed.success) handler(parsed.data);
  });
}
