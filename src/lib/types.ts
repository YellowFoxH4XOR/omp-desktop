export type HarnessKind = 'pi';
export type ThreadStatus = 'active' | 'waiting' | 'idle' | 'completed' | 'failed' | 'disconnected';
export type ToolStatus = 'queued' | 'running' | 'completed' | 'failed' | 'cancelled';

export interface HarnessInstallation {
  kind: HarnessKind;
  path: string;
  version: string;
  source: string;
}
export type InstallStage = 'preparing' | 'installing' | 'verifying';
export type InstallStatus = 'idle' | InstallStage | 'complete' | 'failed';
export interface HarnessInstallCommand {
  kind: HarnessKind;
  command: string;
  installPath: string;
  agentDir: string;
  loginCommand: string;
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
  /** Plan is read-only until the user approves a plan; Auto has full tools. */
  mode: ThreadMode;
}
export type ThreadMode = 'plan' | 'auto';
export interface ThreadDeletePreview {
  hasSession: boolean;
  worktreePath?: string;
  changedFiles: number;
}
export interface ModelInfo {
  provider: string;
  id: string;
  name: string;
  contextWindow?: number;
  reasoning?: boolean;
  maxTokens?: number;
  /** Accepts image input. */
  images?: boolean;
  /** USD per million tokens. */
  cost?: { input: number; output: number };
}
export interface ModelDefaults {
  provider?: string | null;
  modelId?: string | null;
  thinkingLevel?: string | null;
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
  /** Pi's runnable `/` commands (extensions, skills, prompt templates). */
  commands?: Array<{ name: string; description?: string; source: 'extension' | 'skill' | 'prompt' }>;
}
export interface ToolResult { content?: RpcContent[]; details?: unknown; isError?: boolean }
export type ConversationItem =
  | { id: string; kind: 'user' | 'text' | 'thinking'; text: string; streaming?: boolean; timestamp?: number }
  | { id: string; kind: 'tool'; toolCallId: string; toolName: string; args: Record<string, unknown>; intent?: string; status: ToolStatus; result?: ToolResult; partial?: ToolResult; timestamp?: number }
  | { id: string; kind: 'custom' | 'notice' | 'advisor'; text: string; customType?: string; details?: unknown; level?: string; timestamp?: number };
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
  commands: Array<{ name: string; description?: string; source?: 'extension' | 'skill' | 'prompt' }>;
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
export interface InternImage { data: string; mimeType: string }
export interface InternPlan {
  id: string;
  threadId: string;
  threadTitle: string;
  projectPath: string | null;
  summary: string;
  actions: Array<Record<string, unknown>>;
  details: Array<Record<string, unknown>>;
  executing: boolean;
}
export type BackendEvent =
  | { type: 'intern_changed' }
  | { type: 'rpc'; threadId: string; frame: Record<string, unknown> }
  | { type: 'exited'; threadId: string; code?: number; stderr: string; expected: boolean }
  | { type: 'git_changed'; threadId: string }
  | { type: 'install_stage'; kind: HarnessKind; stage: InstallStage }
  | { type: 'install_progress'; kind: HarnessKind; line: string }
  | { type: 'install_finished'; kind: HarnessKind; success: boolean; error?: string };
export interface ThreadRuntime {
  threadId: string;
  projectId: string;
  title: string;
  pid?: number | null;
  memoryBytes?: number | null;
  processCount: number;
  busy: boolean;
  idleSeconds: number;
}
export interface RuntimeStats {
  appBytes?: number | null;
  threads: ThreadRuntime[];
}
export type PackageKind = 'extension' | 'skill' | 'theme' | 'prompt';
export interface CatalogPackage {
  name: string;
  description: string;
  author: string;
  downloadsLabel: string;
  downloads: number;
  publishedMs: number;
  types: PackageKind[];
  /** Source for `pi install`, e.g. `npm:pi-mcp-adapter`. */
  source: string;
}
export interface CatalogPage {
  packages: CatalogPackage[];
  page: number;
  hasMore: boolean;
}
export interface InstalledPackage {
  source: string;
  name: string;
  kind: 'npm' | 'git' | 'local';
  version?: string;
  pinned?: string;
  description?: string;
  latest?: string;
  updateAvailable: boolean;
}
export interface McpServer {
  name: string;
  transport: 'stdio' | 'http' | 'socket' | 'unknown';
  target: string;
  disabled: boolean;
  auth?: string;
  lifecycle?: string;
  hasSecrets: boolean;
  /** Full entry (private servers only). */
  config?: Record<string, unknown>;
}
export interface McpImportSource {
  id: string;
  path: string;
  servers: McpServer[];
}
export interface McpOverview {
  adapterInstalled: boolean;
  adapterVersion?: string;
  path: string;
  servers: McpServer[];
  approveTools: 'off' | 'all' | 'custom';
  raw: string;
  hasComments: boolean;
  error?: string;
  importSources: McpImportSource[];
}
