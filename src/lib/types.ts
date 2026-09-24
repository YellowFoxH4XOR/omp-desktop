export type HarnessKind = 'omp' | 'pi';
export type ThreadStatus = 'active' | 'waiting' | 'idle' | 'completed' | 'failed' | 'disconnected';
export type ToolStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';

export interface HarnessInstallation {
  kind: HarnessKind;
  path: string;
  version: string;
  source: string;
}
export interface Project {
  id: string;
  path: string;
  displayName: string;
  preferredHarness: HarnessKind;
  isGit: boolean;
  createdAt: string;
  lastOpenedAt: string;
}
export interface Thread {
  id: string;
  projectId: string;
  harness: HarnessKind;
  sessionId: string;
  sessionFile: string;
  cwd: string;
  title: string;
  pinned: boolean;
  archived: boolean;
  status: ThreadStatus;
  createdAt: string;
  lastViewedAt: string;
  worktreePath?: string;
}
export interface ModelInfo {
  provider: string;
  id: string;
  name: string;
  contextWindow?: number;
  reasoning?: boolean;
}
export interface LoginProvider {
  id: string;
  name: string;
  available: boolean;
  authenticated: boolean;
}
export interface ContextUsage { tokens: number | null; contextWindow: number; percent: number | null }
export interface Usage {
  tokens: { input: number; output: number; cacheRead: number; cacheWrite: number; total: number };
  cost: number;
  contextUsage?: ContextUsage;
}
export interface HarnessCapabilities {
  agents: boolean;
  nestedAgents: boolean;
  agentSteering: boolean;
  agentKill: boolean;
  agentRevive: boolean;
  planMode: boolean;
  permissions: boolean;
  modelSwitching: boolean;
  effortLevels: boolean;
  contextUsage: boolean;
  tokenUsage: boolean;
  worktrees: boolean;
}
export interface SessionState {
  sessionId: string;
  sessionFile?: string;
  sessionName?: string;
  model?: ModelInfo;
  thinkingLevel?: string;
  isStreaming: boolean;
  contextUsage?: ContextUsage;
}
export interface RpcContent { type: string; text?: string; thinking?: string; id?: string; name?: string; arguments?: Record<string, unknown>; [key: string]: unknown }
export interface RpcMessage { role: string; content?: string | RpcContent[]; timestamp?: number; customType?: string; display?: boolean; details?: unknown; toolCallId?: string; toolName?: string; isError?: boolean; usage?: Record<string, unknown>; stopReason?: string; command?: string; output?: string; exitCode?: number }
export interface SessionSnapshot {
  thread: Thread;
  messages: RpcMessage[];
  state: SessionState;
  models: ModelInfo[];
  levels: string[];
  capabilities: HarnessCapabilities;
  agents: AgentInfo[];
}
export interface ToolResult { content?: RpcContent[]; details?: unknown; isError?: boolean }
export type ConversationItem =
  | { id: string; kind: 'user' | 'text' | 'thinking'; text: string; streaming?: boolean; timestamp?: number }
  | { id: string; kind: 'tool'; toolCallId: string; toolName: string; args: Record<string, unknown>; intent?: string; status: ToolStatus; result?: ToolResult; partial?: ToolResult; timestamp?: number }
  | { id: string; kind: 'custom' | 'notice' | 'advisor'; text: string; customType?: string; details?: unknown; level?: string; timestamp?: number };
export interface AgentInfo {
  id: string;
  parentId?: string;
  parentToolCallId?: string;
  name: string;
  role?: string;
  task?: string;
  status: 'pending' | 'running' | 'waiting' | 'completed' | 'failed' | 'aborted' | 'parked';
  model?: string;
  effort?: string;
  activity?: string;
  tokens?: number;
  contextTokens?: number;
  contextWindow?: number;
  cost?: number;
  durationMs?: number;
  toolCount?: number;
  sessionFile?: string;
  worktreePath?: string;
}
export interface UiRequest {
  id: string;
  method: 'permission' | 'select' | 'confirm' | 'input' | 'editor' | 'open_url';
  title: string;
  message?: string;
  options?: string[];
  optionDetails?: Array<{ description?: string }>;
  placeholder?: string;
  prefill?: string;
  timeout?: number;
  url?: string;
  launchUrl?: string;
  toolName?: string;
  toolArgs?: Record<string, unknown>;
  cwd?: string;
}
export interface UiResponse { value?: string; confirmed?: boolean; cancelled?: boolean }
export interface SessionView {
  threadId: string;
  items: ConversationItem[];
  agents: AgentInfo[];
  pendingRequests: UiRequest[];
  status: ThreadStatus;
  capabilities: HarnessCapabilities;
  model?: ModelInfo;
  effort?: string;
  contextUsage?: ContextUsage;
  usage?: Usage;
  error?: string;
  sessionFile?: string;
  models: ModelInfo[];
  levels: string[];
  commands: Array<{ name: string; description?: string }>;
}
export interface ChangedFile {
  path: string;
  status: string;
  additions: number;
  deletions: number;
  binary: boolean;
}
export interface ChangesSummary {
  isRepo: boolean;
  branch?: string;
  files: ChangedFile[];
  additions: number;
  deletions: number;
}
export interface GitFile {
  path: string;
  old: string;
  current: string;
  currentHash: string;
  binary: boolean;
  tooLarge: boolean;
}
export type BackendEvent =
  | { type: 'rpc'; threadId: string; frame: Record<string, unknown> }
  | { type: 'exited'; threadId: string; code?: number; stderr: string; expected: boolean }
  | { type: 'git_changed'; threadId: string }
  | { type: 'install_progress'; kind: HarnessKind; line: string }
  | { type: 'install_finished'; kind: HarnessKind; success: boolean; error?: string };
