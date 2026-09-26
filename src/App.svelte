<script lang="ts">
  import './app.css';
  import { onMount, tick, untrack } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import { VList } from 'virtua/svelte';
  import { Bot, Cpu, House, Plug, Puzzle, SlidersHorizontal, Plus, Settings2, GitCompareArrows, Search, ChevronRight, X, Pin, Archive, MoreHorizontal, RefreshCw, Terminal, FolderPlus, Folder, AlertTriangle, LoaderCircle, Check, SquarePen, GitFork, Trash2, MessageSquare, Monitor, Sun, Moon } from '@lucide/svelte';
  import { api, onBackendEvent } from '$lib/api';
  import { SessionModel } from '$lib/session.svelte';
  import Conversation from '$lib/components/conversation/Conversation.svelte';
  import PiSetup from '$lib/components/setup/PiSetup.svelte';
  import PiSignIn from '$lib/components/setup/PiSignIn.svelte';
  import RuntimeMonitor from '$lib/components/runtime/RuntimeMonitor.svelte';
  import ModelPicker from '$lib/components/conversation/ModelPicker.svelte';
  import { modelKey } from '$lib/components/conversation/model-utils';
  import PiTerminal from '$lib/components/terminal/PiTerminal.svelte';
  import PiIntern from '$lib/components/intern/PiIntern.svelte';
  import Welcome from '$lib/components/welcome/Welcome.svelte';
  import PlanReview from '$lib/components/plan/PlanReview.svelte';
  import CountUp from '$lib/components/common/CountUp.svelte';
  import Extensions from '$lib/components/settings/Extensions.svelte';
  import McpServers from '$lib/components/settings/McpServers.svelte';
  import { openExternal } from '$lib/components/conversation/links';
  import { checkForUpdates } from '$lib/extension-updates.svelte';
  import { parseSlash, TERMINAL_ONLY } from '$lib/slash';
  import { ago, clock } from '$lib/time';
  import { planOf, PLAN_APPROVE, PLAN_DECLINE, PLAN_FEEDBACK } from '$lib/plan';
  import ConfirmDialog from '$lib/components/diff/ConfirmDialog.svelte';
  import type { BackendEvent, HarnessInstallation, HarnessInstallCommand, InstallStatus, ModelDefaults, ModelInfo, Project, SessionSnapshot, SessionState, Thread, ThreadDeletePreview, ThreadMode, ThreadStatus, UiResponse } from '$lib/types';
  type ChangesPanelComponent = (typeof import('$lib/components/diff/ChangesPanel.svelte'))['default'];
  type RpcFrame = Record<string, unknown>;
  interface OpenedSession {
    model: SessionModel;
    thread: Thread;
  }
  interface PendingSessionOpen {
    projectId: string;
    frames: RpcFrame[];
    estimatedBytes: number;
    discarded: boolean;
    failure?: Error;
    reject?: (reason: Error) => void;
    promise?: Promise<OpenedSession>;
  }

  const MAX_CACHED_SESSIONS = 8;
  const RECENT_THREAD_LIMIT = 10;
  /** A background thread's warm Pi is kept this long; reopening a warm thread is instant. */
  const IDLE_STOP_MS = 10 * 60_000;
  const PREWARM_HOVER_MS = 120;
  const PREWARM_COOLDOWN_MS = 60_000;
  const MAX_PENDING_SESSION_FRAMES = 1_024;
  const MAX_PENDING_SESSION_BYTES = 8 * 1024 * 1024;
  const MAX_INVALIDATED_THREADS = 1_024;
  const MAX_CRASH_DETAILS = 8;
  const MAX_CRASH_DETAIL_BYTES = 64 * 1024;
  const MAX_CRASH_DETAILS_BYTES = 256 * 1024;
  const utf8Encoder = new TextEncoder();

  let projects = $state<Project[]>([]);
  let recentThreads = $state<Thread[]>([]);
  let threadsByProject = $state<Record<string, Thread[]>>({});
  let harnesses = $state<HarnessInstallation[]>([]);
  let detecting = $state(true);
  let startupError = $state('');
  let crashDetails = $state<Record<string, string>>({});
  let errorDetailsOpen = $state(false);
  let activeProject = $state<Project | null>(null);
  let activeThread = $state<Thread | null>(null);
  let selectedThreadId = $state<string | null>(null);
  let activeSession = $state<SessionModel | null>(null);
  let loadingThread = $state(false);
  let pendingAction = $state(false);
  let pendingActionCount = 0;
  let rightPanel = $state<'changes' | 'plan' | null>(null);
  let ChangesPanel = $state<ChangesPanelComponent | null>(null);
  let diffPath = $state<string | undefined>(undefined);
  let sidebarWidth = $state(256);
  let panelWidth = $state(405);
  let settingsOpen = $state(false);
  type SettingsSection = 'general' | 'models' | 'pi' | 'extensions' | 'mcp';
  let settingsSection = $state<SettingsSection>('general');
  const SETTINGS_SECTIONS: Array<{ id: SettingsSection; label: string; icon: typeof Cpu }> = [
    { id: 'general', label: 'General', icon: SlidersHorizontal },
    { id: 'models', label: 'Models', icon: Cpu },
    { id: 'pi', label: 'Pi runtime', icon: Terminal },
    { id: 'extensions', label: 'Extensions', icon: Puzzle },
    { id: 'mcp', label: 'MCP servers', icon: Plug },
  ];
  /** Installed Pi packages with a newer npm release; badges Settings. */
  let extensionUpdates = $state(0);
  let terminalOpen = $state(false);
  let internOpen = $state(false);
  let internStarted = $state(false);
  let internPending = $state(0);
  let internRefreshEpoch = 0;
  let copiedPath = $state(false);
  let switcherOpen = $state(false);
  let switchQuery = $state('');
  let switchIndex = $state(0);
  let showArchived = $state(false);
  let theme = $state<'system' | 'dark' | 'light'>('system');
  let installInProgress = $state(false);
  let installStatus = $state<InstallStatus>('idle');
  let installLog = $state<string[]>([]);
  let installError = $state('');
  let installPlan = $state<HarnessInstallCommand | null>(null);
  let installPlanError = $state('');
  let installerVisible = $state(false);
  let checkingRuntime = $state(false);
  let eventsReady = $state(false);
  let renaming = $state<string | null>(null);
  let projectMenu = $state<string | null>(null);
  let threadMenu = $state<{ id: string; x: number; y: number } | null>(null);
  let deleteDialog = $state<{ thread: Thread; preview: ThreadDeletePreview; busy: boolean } | null>(null);
  let previewingDelete = $state(false);
  let collapsedProjects = $state<Set<string>>(new Set());
  let renameText = $state('');
  let sidebarResizing = false;
  let panelResizing = false;
  const liveSessions = new Map<string, SessionModel>();
  const idleStopTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const sessionProjects = new Map<string, string>();
  const openingSessions = new Map<string, PendingSessionOpen>();
  const invalidatedProjects = new Set<string>();
  const invalidatedThreads = new Set<string>();
  const invalidatedThreadOrder: string[] = [];
  const stopping = new Map<string, Promise<void>>();
  let projectSelectionToken = 0;
  let threadSelectionToken = 0;
  const currentView = $derived(activeSession?.view);
  // Live row details: when each thread started working, a ticking clock, and
  // Git totals for isolated worktrees (a shared checkout's diff isn't one
  // thread's work, so those rows don't claim it).
  let now = $state(Date.now());
  const activeSince = $state<Record<string, number>>({});
  const rowStats = $state<Record<string, { additions: number; deletions: number; branch?: string }>>({});
  const statsTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const MAX_STATS_ROWS = 24;
  $effect(() => {
    const timer = setInterval(() => { now = Date.now(); }, 1000);
    return () => clearInterval(timer);
  });
  $effect(() => {
    for (const rows of Object.values(threadsByProject)) for (const thread of rows ?? []) {
      const tracked = untrack(() => thread.id in activeSince);
      if (thread.status === 'active' && !tracked) activeSince[thread.id] = Date.now();
      else if (thread.status !== 'active' && tracked) delete activeSince[thread.id];
    }
  });
  function refreshStats(thread: Thread, delay = 0) {
    if (!thread.worktreePath) return;
    const pending = statsTimers.get(thread.id);
    if (pending) clearTimeout(pending);
    statsTimers.set(thread.id, setTimeout(() => {
      statsTimers.delete(thread.id);
      void api.gitStatus(thread.id).then(summary => {
        if (summary.isRepo) rowStats[thread.id] = { additions: summary.additions, deletions: summary.deletions, branch: summary.branch };
      }).catch(() => undefined);
    }, delay));
  }
  $effect(() => {
    const project = activeProject;
    if (!project) return;
    const rows = (threadsByProject[project.id] ?? []).filter(thread => thread.worktreePath && !thread.archived).slice(0, MAX_STATS_ROWS);
    for (const thread of rows) if (untrack(() => !(thread.id in rowStats))) refreshStats(thread);
  });
  function findThread(threadId: string): Thread | undefined {
    for (const rows of Object.values(threadsByProject)) { const found = rows?.find(row => row.id === threadId); if (found) return found; }
    return undefined;
  }
  /** A worktree's branch once one exists, else its short folder name. */
  function worktreeLabel(thread: Thread): string {
    const branch = rowStats[thread.id]?.branch;
    if (branch) return branch;
    const folder = thread.worktreePath?.split('/').filter(Boolean).at(-1) ?? '';
    return `worktree ${folder.slice(0, 8)}`;
  }
  async function revealWorktree(path: string) {
    try { await revealItemInDir(path); }
    catch (error) { startupError = `Could not show the worktree: ${errorText(error)}`; }
  }

  /** Amber pill text for a thread that is waiting on the user. */
  function waitingLabel(thread: Thread): string | null {
    if (thread.status !== 'waiting') return null;
    const session = liveSessions.get(thread.id);
    return session?.view.pendingRequests.some(request => planOf(request) !== null) ? 'Needs approval' : 'Needs input';
  }

  // A pending Plan → Auto approval for the shown thread opens the plan panel.
  const planRequest = $derived(currentView?.pendingRequests.find(request => planOf(request) !== null));
  let shownPlanId: string | undefined;
  $effect(() => {
    const id = planRequest?.id;
    if (id && id !== shownPlanId) { shownPlanId = id; rightPanel = 'plan'; }
    else if (!id && rightPanel === 'plan') rightPanel = null;
  });
  const installation = $derived(harnesses[0]);
  const onboarding = $derived(!detecting && !installation);
  const switchEntries = $derived([
    ...projects.flatMap(project => [
      { kind: 'project' as const, label: project.displayName, subtitle: project.path, project },
      ...(threadsByProject[project.id] ?? []).filter(t => !t.archived).map(thread => ({ kind: 'thread' as const, label: thread.title || 'New thread', subtitle: project.displayName, project, thread }))
    ]),
    ...(installation ? [{ kind: 'terminal' as const, label: 'Open Pi terminal', subtitle: 'Private Pi shell' }] : []),
  ].filter(entry => `${entry.label} ${entry.subtitle}`.toLowerCase().includes(switchQuery.toLowerCase())).slice(0, 40));
  const visibleThread = $derived(activeThread && activeProject && activeThread.projectId === activeProject.id ? activeThread : null);
  const installReady = $derived(eventsReady && installPlan !== null);

  function utf8Bytes(value: string): number {
    return utf8Encoder.encode(value).byteLength;
  }
  function jsonStringBytes(value: string, maxBytes: number): number {
    let bytes = 2;
    for (const character of value) {
      const code = character.codePointAt(0)!;
      if (code === 0x22 || code === 0x5c || code === 0x08 || code === 0x0c || code === 0x0a || code === 0x0d || code === 0x09) bytes += 2;
      else if (code < 0x20) bytes += 6;
      else if (code <= 0x7f) bytes += 1;
      else if (code <= 0x7ff) bytes += 2;
      else if (code <= 0xffff) bytes += 3;
      else bytes += 4;
      if (bytes > maxBytes) break;
    }
    return bytes;
  }
  function estimateJsonBytes(value: unknown, maxBytes: number, seen = new WeakSet<object>()): number {
    let bytes = 0;
    let steps = 0;
    const visit = (current: unknown, depth = 0): void => {
      if (bytes > maxBytes || ++steps > 100_000 || depth > 512) { bytes = maxBytes + 1; return; }
      if (current === null) { bytes += 4; return; }
      if (typeof current === 'string') { bytes += jsonStringBytes(current, maxBytes); return; }
      if (typeof current === 'number') { bytes += 24; return; }
      if (typeof current === 'boolean') { bytes += current ? 4 : 5; return; }
      if (current === undefined || typeof current === 'function' || typeof current === 'symbol') return;
      if (typeof current !== 'object') { bytes += 8; return; }
      if (seen.has(current)) return;
      seen.add(current);
      if (Array.isArray(current)) {
        bytes += 2;
        for (let index = 0; index < current.length && bytes <= maxBytes; index++) {
          if (index) bytes += 1;
          visit(current[index], depth + 1);
        }
      } else {
        bytes += 2;
        let first = true;
        for (const key in current) {
          if (!Object.prototype.hasOwnProperty.call(current, key)) continue;
          if (!first) bytes += 1;
          first = false;
          bytes += jsonStringBytes(key, maxBytes) + 1;
          visit((current as Record<string, unknown>)[key], depth + 1);
          if (bytes > maxBytes) break;
        }
      }
    };
    visit(value);
    return bytes;
  }
  function addInvalidatedThread(threadId: string) {
    if (invalidatedThreads.has(threadId)) return;
    invalidatedThreads.add(threadId);
    invalidatedThreadOrder.push(threadId);
    while (invalidatedThreadOrder.length > MAX_INVALIDATED_THREADS) invalidatedThreads.delete(invalidatedThreadOrder.shift()!);
  }
  function removeInvalidatedThread(threadId: string) {
    invalidatedThreads.delete(threadId);
    const index = invalidatedThreadOrder.indexOf(threadId);
    if (index >= 0) invalidatedThreadOrder.splice(index, 1);
  }
  function truncateUtf8(value: string, maxBytes: number): string {
    if (utf8Bytes(value) <= maxBytes) return value;
    let low = 0;
    let high = Math.min(value.length, maxBytes);
    while (low < high) {
      const middle = Math.ceil((low + high) / 2);
      if (utf8Bytes(value.slice(0, middle)) <= maxBytes) low = middle;
      else high = middle - 1;
    }
    return value.slice(0, low);
  }
  function recordCrashDetails(threadId: string, stderr: string) {
    if (invalidatedThreads.has(threadId)) return;
    const next = { ...crashDetails };
    delete next[threadId];
    next[threadId] = truncateUtf8(stderr, MAX_CRASH_DETAIL_BYTES);
    let keys = Object.keys(next);
    while (keys.length > MAX_CRASH_DETAILS) {
      delete next[keys.shift()!];
      keys = Object.keys(next);
    }
    let totalBytes = Object.values(next).reduce((total, value) => total + utf8Bytes(value), 0);
    while (totalBytes > MAX_CRASH_DETAILS_BYTES && keys.length > 1) {
      const oldest = keys.shift()!;
      totalBytes -= utf8Bytes(next[oldest]);
      delete next[oldest];
    }
    crashDetails = next;
  }
  function errorText(error: unknown): string { return error instanceof Error ? error.message : String(error); }
  function beginPendingAction() {
    pendingActionCount += 1;
    pendingAction = true;
  }
  function endPendingAction() {
    pendingActionCount = Math.max(0, pendingActionCount - 1);
    pendingAction = pendingActionCount > 0;
  }
  function isSessionInactive(model: SessionModel): boolean {
    const view = model.view;
    return view.status !== 'active' && view.status !== 'waiting';
  }
  function removeCachedSession(threadId: string, expected?: SessionModel) {
    const model = liveSessions.get(threadId);
    if (!model || (expected && model !== expected)) return false;
    liveSessions.delete(threadId);
    sessionProjects.delete(threadId);
    return true;
  }
  function cacheSession(threadId: string, projectId: string, model: SessionModel) {
    liveSessions.delete(threadId);
    liveSessions.set(threadId, model);
    sessionProjects.set(threadId, projectId);
    enforceSessionLimit();
  }
  function stopInactiveSession(threadId: string, expected: SessionModel): boolean {
    if (activeThread?.id === threadId || openingSessions.has(threadId) || stopping.has(threadId) || !isSessionInactive(expected)) return false;
    cancelIdleStop(threadId);
    // Keep the cached session until the stop succeeds so live output still routes.
    let job: Promise<void>;
    job = api.stopThread(threadId).then(() => {
      removeCachedSession(threadId, expected);
    }).catch((error: unknown) => {
      startupError = `Could not stop idle session: ${errorText(error)}`;
      if (liveSessions.get(threadId) === expected) scheduleIdleStop(threadId);
    }).finally(() => {
      if (stopping.get(threadId) === job) stopping.delete(threadId);
    });
    stopping.set(threadId, job);
    return true;
  }
  function failPendingSessionOpen(threadId: string, pending: PendingSessionOpen, message: string) {
    if (pending.failure) return;
    const failure = new Error(message);
    pending.failure = failure;
    pending.frames.length = 0;
    pending.estimatedBytes = 0;
    pending.reject?.(failure);
    void api.stopThread(threadId).catch(() => undefined);
  }
  function enforceSessionLimit() {
    let excess = liveSessions.size - MAX_CACHED_SESSIONS;
    for (const [threadId, model] of liveSessions) {
      if (excess <= 0) return;
      if (stopInactiveSession(threadId, model)) excess -= 1;
    }
  }
  function applyRpcFrame(threadId: string, frame: RpcFrame) {
    const model = liveSessions.get(threadId);
    model?.apply(frame);
    const kind = frame.type;
    if (kind === 'agent_start') updateThreadStatus(threadId, 'active');
    if (kind === 'response' && frame.success === false && model?.view.status === 'failed') {
      updateThreadStatus(threadId, 'failed');
    }
    // The backend settles Pi only after checking queued and extension work.
    if (kind === 'agent_settled') {
      updateThreadStatus(threadId, model?.view.status === 'failed' ? 'failed' : 'completed');
    }
    if (kind === 'extension_ui_request' && ['select', 'confirm', 'input', 'editor'].includes(String(frame.method))) updateThreadStatus(threadId, 'waiting');
    if (model && activeThread?.id !== threadId && isSessionInactive(model)) scheduleIdleStop(threadId);
    enforceSessionLimit();
  }
  function openSessionModel(thread: Thread, optimistic?: SessionModel): Promise<OpenedSession> {
    const existing = openingSessions.get(thread.id);
    if (existing?.promise) return existing.promise;
    const pending: PendingSessionOpen = { projectId: thread.projectId, frames: [], estimatedBytes: 0, discarded: false };
    openingSessions.set(thread.id, pending);
    const promise = new Promise<OpenedSession>((resolve, reject) => {
      pending.reject = reject;
      void api.openThread(thread.id).then(snapshot => {
        if (pending.failure || pending.discarded || invalidatedProjects.has(pending.projectId)) {
          if (openingSessions.get(thread.id) === pending) openingSessions.delete(thread.id);
          pending.frames.length = 0;
          pending.estimatedBytes = 0;
          throw pending.failure ?? new Error('Thread opening was cancelled because its project was removed.');
        }
        if (invalidatedThreads.has(thread.id)) throw new Error('Thread opening was cancelled because the thread is no longer available.');
        let model: SessionModel;
        if (optimistic) {
          optimistic.reconnect(snapshot);
          model = optimistic;
        } else {
          model = new SessionModel(snapshot);
        }
        rememberSessionMeta(snapshot);
        liveSessions.delete(thread.id);
        liveSessions.set(thread.id, model);
        sessionProjects.set(thread.id, snapshot.thread.projectId);
        for (const frame of pending.frames.splice(0)) applyRpcFrame(thread.id, frame);
        pending.estimatedBytes = 0;
        if (openingSessions.get(thread.id) === pending) openingSessions.delete(thread.id);
        enforceSessionLimit();
        resolve({ model, thread: snapshot.thread });
      }).catch(error => {
        if (openingSessions.get(thread.id) === pending) openingSessions.delete(thread.id);
        pending.frames.length = 0;
        pending.estimatedBytes = 0;
        reject(error);
      });
    });
    pending.promise = promise;
    return promise;
  }
  /** Model/effort metadata from the last opened thread; a brand-new thread has
   *  no history, so it can render immediately with these while Pi starts. */
  let sessionMeta: Pick<SessionSnapshot, 'models' | 'levels' | 'capabilities'> & { state: SessionState } | null = null;
  const MODEL_CACHE_KEY = 'pidesk.models';
  const THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
  function cachedModels(): ModelInfo[] {
    try {
      const parsed: unknown = JSON.parse(localStorage.getItem(MODEL_CACHE_KEY) ?? '[]');
      return Array.isArray(parsed) ? parsed.filter((model): model is ModelInfo => !!model && typeof model.provider === 'string' && typeof model.id === 'string').slice(0, 500) : [];
    } catch { return []; }
  }
  /** Latest model catalogue, kept so Settings can pick a default before any thread opens. */
  let knownModels = $state<ModelInfo[]>(cachedModels());
  let modelDefaults = $state<ModelDefaults | null>(null);
  const defaultModelKey = $derived(modelDefaults?.provider && modelDefaults.modelId ? `${modelDefaults.provider}/${modelDefaults.modelId}` : null);
  async function loadModelDefaults() {
    try { modelDefaults = await api.getModelDefaults(); } catch { modelDefaults = null; }
  }
  async function makeDefaultModel(model: ModelInfo) {
    try { modelDefaults = await api.setDefaultModel(model.provider, model.id); }
    catch (error) { startupError = `Could not set the default model: ${errorText(error)}`; }
  }
  async function setDefaultEffort(level: string) {
    try { modelDefaults = await api.setDefaultThinkingLevel(level); }
    catch (error) { startupError = `Could not set the default effort: ${errorText(error)}`; }
  }
  function rememberSessionMeta(snapshot: SessionSnapshot) {
    if (snapshot.models.length) {
      knownModels = snapshot.models;
      try { localStorage.setItem(MODEL_CACHE_KEY, JSON.stringify(snapshot.models.slice(0, 500))); } catch { /* best effort */ }
    }
    sessionMeta = {
      models: snapshot.models,
      levels: snapshot.levels,
      capabilities: snapshot.capabilities,
      state: { sessionId: '', isStreaming: false, model: snapshot.state.model, thinkingLevel: snapshot.state.thinkingLevel },
    };
  }
  function optimisticSession(thread: Thread): SessionModel | null {
    if (!sessionMeta || thread.sessionId || thread.sessionFile) return null;
    // New threads start on the configured default, exactly as Pi will.
    const preferred = sessionMeta.models.find(model => modelKey(model) === defaultModelKey);
    const state: SessionState = {
      ...sessionMeta.state,
      model: preferred ?? sessionMeta.state.model,
      thinkingLevel: modelDefaults?.thinkingLevel ?? sessionMeta.state.thinkingLevel,
    };
    return new SessionModel({ thread, messages: [], ...sessionMeta, state });
  }
  const prewarmedAt = new Map<string, number>();
  let prewarmTimer: ReturnType<typeof setTimeout> | undefined;
  /** Hovering a thread starts its Pi so the click lands on a warm process. */
  function prewarmSoon(thread: Thread) {
    clearTimeout(prewarmTimer);
    if (!installation || liveSessions.has(thread.id) || openingSessions.has(thread.id) || stopping.has(thread.id)) return;
    const last = prewarmedAt.get(thread.id);
    if (last !== undefined && Date.now() - last < PREWARM_COOLDOWN_MS) return;
    prewarmTimer = setTimeout(() => {
      if (prewarmedAt.size > 256) prewarmedAt.clear();
      prewarmedAt.set(thread.id, Date.now());
      void api.prewarmThread(thread.id).catch(() => undefined);
    }, PREWARM_HOVER_MS);
  }
  function cancelPrewarm() {
    clearTimeout(prewarmTimer);
  }
  function visibleThreads(projectId: string): Thread[] {
    return (threadsByProject[projectId] ?? []).filter(t => showArchived || !t.archived)
      .sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.lastViewedAt.localeCompare(a.lastViewedAt));
  }
  function updateThreadStatus(threadId: string, status: ThreadStatus) {
    for (const projectId of Object.keys(threadsByProject)) {
      const thread = threadsByProject[projectId]?.find(t => t.id === threadId);
      if (thread) { thread.status = status; if (activeThread?.id === threadId) activeThread.status = status; break; }
    }
  }
  function scheduleIdleStop(threadId: string) {
    if (idleStopTimers.has(threadId)) return;
    const expected = liveSessions.get(threadId);
    if (!expected) return;
    const timer = setTimeout(() => {
      idleStopTimers.delete(threadId);
      if (activeThread?.id !== threadId && isSessionInactive(expected)) {
        stopInactiveSession(threadId, expected);
      }
    }, IDLE_STOP_MS);
    idleStopTimers.set(threadId, timer);
  }
  function leaveCurrentThread(nextId?: string) {
    if (!activeThread || activeThread.id === nextId) return;
    const model = liveSessions.get(activeThread.id);
    if (model && isSessionInactive(model)) scheduleIdleStop(activeThread.id);
  }
  function cancelIdleStop(threadId: string) {
    const timer = idleStopTimers.get(threadId);
    if (timer) clearTimeout(timer);
    idleStopTimers.delete(threadId);
  }
  function handleBackendEvent(event: BackendEvent) {
    if (event.type === 'intern_changed') {
      void refreshInternState();
      if (!internStarted) void api.internPlans().then(plans => { internPending = plans.filter(plan => !plan.executing).length; }).catch(() => undefined);
      return;
    }
    if ('threadId' in event && invalidatedThreads.has(event.threadId)) return;
    if (event.type === 'git_changed') { const thread = findThread(event.threadId); if (thread) refreshStats(thread, 800); }
    if (event.type === 'rpc') {
      const pending = openingSessions.get(event.threadId);
      if (pending) {
        if (pending.failure) return;
        const frameBytes = estimateJsonBytes(event.frame, MAX_PENDING_SESSION_BYTES);
        if (pending.frames.length >= MAX_PENDING_SESSION_FRAMES || pending.estimatedBytes + frameBytes > MAX_PENDING_SESSION_BYTES) {
          failPendingSessionOpen(event.threadId, pending, 'Thread opening received too many events and was stopped.');
          return;
        }
        pending.frames.push(event.frame);
        pending.estimatedBytes += frameBytes;
      } else {
        applyRpcFrame(event.threadId, event.frame);
      }
    } else if (event.type === 'exited') {
      if (!event.expected) {
        recordCrashDetails(event.threadId, event.stderr);
        liveSessions.get(event.threadId)?.setError('Pi stopped unexpectedly. Your visible conversation is preserved.');
        updateThreadStatus(event.threadId, 'disconnected');
      } else {
        updateThreadStatus(event.threadId, 'idle');
        if (activeThread?.id !== event.threadId) removeCachedSession(event.threadId);
      }
    } else if (event.type === 'install_stage' && installInProgress) {
      installStatus = event.stage;
    } else if (event.type === 'install_progress' && installInProgress) {
      appendInstallLog(event.line);
    } else if (event.type === 'install_finished' && installInProgress && !event.success) {
      installError = event.error || 'Pi could not be installed. Review the output and try again.';
      installStatus = 'failed';
    }
  }
  function appendInstallLog(line: string) {
    installLog = [...installLog, ...truncateUtf8(line, 4096).split(/\r?\n/)].slice(-200);
  }
  onMount(() => {
    sidebarWidth = Number(localStorage.getItem('sidebarWidth')) || 256;
    panelWidth = Number(localStorage.getItem('panelWidth')) || 405;
    const savedTheme = localStorage.getItem('theme');
    if (savedTheme === 'light' || savedTheme === 'dark') theme = savedTheme;
    applyTheme();
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onBackendEvent(handleBackendEvent).then(fn => {
      if (disposed) fn();
      else { unlisten = fn; eventsReady = true; }
    }).catch(err => { if (!disposed) startupError = `Could not connect live events. Relaunch πDesk: ${errorText(err)}`; });
    const runtimeReady = checkPrivateRuntime();
    void loadModelDefaults();
    void Promise.allSettled([runtimeReady, refreshProjects()]).then(() => { detecting = false; if (installation) void checkExtensionUpdates(); });
    const onKey = (event: KeyboardEvent) => {
      if (terminalOpen || deleteDialog) return; // Terminal and ConfirmDialog own their keys.
      if (!event.metaKey && event.key !== 'Escape') return;
      const key = event.key.toLowerCase();
      if (event.metaKey && key === 'k') { event.preventDefault(); void openSwitcher(); }
      else if (event.metaKey && key === 'n') { event.preventDefault(); if (activeProject) void createThread(activeProject); }
      else if (event.metaKey && event.shiftKey && key === 'd') { event.preventDefault(); togglePanel('changes'); }
      else if (event.metaKey && key === ',') { event.preventDefault(); void openSettings(); }
      else if (event.key === 'Escape') {
        if (errorDetailsOpen) errorDetailsOpen = false;
        else if (renaming) renaming = null;
        else if (threadMenu) threadMenu = null;
        else if (projectMenu) projectMenu = null;
        else if (switcherOpen) switcherOpen = false;
        else if (settingsOpen) settingsOpen = false;
        else if (rightPanel) rightPanel = null;
        else if (activeSession?.view.status === 'active' && activeThread) void api.abortThread(activeThread.id);
      }
    };
    window.addEventListener('keydown', onKey);
    const move = (event: MouseEvent) => {
      if (sidebarResizing) sidebarWidth = Math.max(205, Math.min(390, event.clientX));
      if (panelResizing) panelWidth = Math.max(320, Math.min(850, innerWidth - event.clientX));
    };
    const up = () => {
      if (sidebarResizing) localStorage.setItem('sidebarWidth', String(sidebarWidth));
      if (panelResizing) localStorage.setItem('panelWidth', String(panelWidth));
      sidebarResizing = false; panelResizing = false;
    };
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    const onWindowError = (event: ErrorEvent) => {
      // Benign layout-timing warning from ResizeObserver, not an app failure.
      if (event.message?.startsWith('ResizeObserver loop')) return;
      startupError = event.message || 'An unexpected error occurred.';
    };
    const onWindowRejection = (event: PromiseRejectionEvent) => {
      const reason: unknown = event.reason;
      startupError = reason instanceof Error ? reason.message : String(reason);
    };
    window.addEventListener('error', onWindowError);
    window.addEventListener('unhandledrejection', onWindowRejection);
    return () => { disposed = true; unlisten?.(); window.removeEventListener('keydown', onKey); window.removeEventListener('mousemove', move); window.removeEventListener('mouseup', up); window.removeEventListener('error', onWindowError); window.removeEventListener('unhandledrejection', onWindowRejection); for (const timer of idleStopTimers.values()) clearTimeout(timer); idleStopTimers.clear(); };
  });
  function applyTheme() { document.documentElement.dataset.theme = theme === 'system' ? '' : theme; localStorage.setItem('theme', theme); }
  async function refreshHarnesses() {
    // Older/system runtime responses must not bypass the private setup screen.
    harnesses = (await api.detectHarnesses()).filter(candidate => candidate.kind === 'pi' && candidate.source === 'managed');
  }
  async function checkPrivateRuntime() {
    if (checkingRuntime || installInProgress) return;
    checkingRuntime = true;
    installPlanError = '';
    try {
      const plans = await api.harnessInstallCommands();
      installPlan = plans.find(plan => plan.kind === 'pi' && plan.installPath && plan.agentDir && plan.loginCommand && plan.command) ?? null;
      if (!installPlan) throw new Error('Private installer information is unavailable. Relaunch or update πDesk.');
      await refreshHarnesses();
      if (installation && installerVisible) { installStatus = 'complete'; installError = ''; }
    } catch (error) {
      installPlanError = errorText(error);
    } finally { checkingRuntime = false; }
  }
  /** Launch lands on the welcome screen; nothing is selected and no Pi starts. */
  async function refreshProjects() {
    try {
      projects = await api.listProjects();
      await loadRecentThreads();
    } catch (error) { startupError = `Could not load projects: ${errorText(error)}`; }
  }
  async function loadRecentThreads() {
    try { recentThreads = await api.listRecentThreads(RECENT_THREAD_LIMIT); }
    catch { recentThreads = []; }
  }
  /** Leaves the current project and thread and shows the welcome screen. */
  function goHome() {
    leaveCurrentThread();
    ++projectSelectionToken; ++threadSelectionToken;
    activeProject = null; activeThread = null; activeSession = null; selectedThreadId = null;
    loadingThread = false; rightPanel = null; diffPath = undefined; errorDetailsOpen = false;
    renaming = null; threadMenu = null; renameText = '';
    localStorage.removeItem('lastProject'); localStorage.removeItem('lastThread');
    void loadRecentThreads();
  }
  async function refreshThreads(projectId: string, selectionToken?: number): Promise<Thread[] | null> {
    try {
      const threads = await api.listThreads(projectId);
      if (invalidatedProjects.has(projectId)) return null;
      if (selectionToken === undefined || selectionToken === projectSelectionToken) threadsByProject[projectId] = threads;
      return threads;
    } catch (error) {
      if (selectionToken === undefined || selectionToken === projectSelectionToken) startupError = `Could not list threads: ${errorText(error)}`;
      return null;
    }
  }
  function expandProject(projectId: string) {
    if (!collapsedProjects.has(projectId)) return;
    const next = new Set(collapsedProjects);
    next.delete(projectId);
    collapsedProjects = next;
  }
  /** Clicking the open project collapses it; any other project is selected and expanded. */
  function toggleProject(project: Project) {
    if (activeProject?.id !== project.id) {
      void selectProject(project);
      return;
    }
    const next = new Set(collapsedProjects);
    if (next.has(project.id)) next.delete(project.id);
    else next.add(project.id);
    collapsedProjects = next;
  }
  async function stopThreadsFromMonitor(threadIds: string[]) {
    const failures: string[] = [];
    await Promise.all(threadIds.map(async threadId => {
      cancelIdleStop(threadId);
      const inflight = stopping.get(threadId);
      if (inflight) { await inflight; return; }
      const job = api.stopThread(threadId).then(() => {
        if (activeThread?.id !== threadId) removeCachedSession(threadId);
      });
      const tracked: Promise<void> = job.catch(() => undefined).finally(() => {
        if (stopping.get(threadId) === tracked) stopping.delete(threadId);
      });
      stopping.set(threadId, tracked);
      try { await job; } catch (error) { failures.push(errorText(error)); }
    }));
    if (failures.length) startupError = `Could not stop ${failures.length === 1 ? 'a thread' : `${failures.length} threads`}: ${failures[0]}`;
  }
  async function openThreadById(threadId: string, projectId: string) {
    if (threadId === 'pidesk-intern') { internStarted = true; internOpen = true; return; }
    const project = projects.find(candidate => candidate.id === projectId);
    if (!project) return;
    let thread = threadsByProject[projectId]?.find(candidate => candidate.id === threadId);
    if (!thread) thread = (await refreshThreads(projectId))?.find(candidate => candidate.id === threadId);
    if (!thread) return;
    if (activeProject?.id !== projectId) {
      const selectionToken = beginProjectSelection(project);
      void refreshThreads(projectId, selectionToken);
    }
    await selectThread(thread);
  }
  function beginProjectSelection(project: Project): number {
    expandProject(project.id);
    leaveCurrentThread();
    const selectionToken = ++projectSelectionToken;
    ++threadSelectionToken;
    activeProject = project;
    activeThread = null;
    errorDetailsOpen = false;
    renaming = null;
    threadMenu = null;
    renameText = '';
    selectedThreadId = null;
    activeSession = null;
    loadingThread = false;
    rightPanel = null;
    diffPath = undefined;
    localStorage.setItem('lastProject', project.id);
    return selectionToken;
  }
  async function selectProject(project: Project) {
    const selectionToken = beginProjectSelection(project);
    await refreshThreads(project.id, selectionToken);
  }
  function clearProjectCache(projectId: string) {
    const threadIds = new Set((threadsByProject[projectId] ?? []).map(thread => thread.id));
    for (const [threadId, cachedProjectId] of sessionProjects) if (cachedProjectId === projectId) threadIds.add(threadId);
    for (const [threadId, pending] of openingSessions) {
      if (pending.projectId !== projectId) continue;
      pending.discarded = true;
      threadIds.add(threadId);
      addInvalidatedThread(threadId);
      pending.reject?.(new Error('Thread opening was cancelled because its project was removed.'));
    }
    delete threadsByProject[projectId];
    for (const threadId of threadIds) {
      addInvalidatedThread(threadId);
      cancelIdleStop(threadId);
      removeCachedSession(threadId);
      delete crashDetails[threadId];
    }
  }
  async function addProject() {
    const projectToken = projectSelectionToken;
    const threadToken = threadSelectionToken;
    const result = await open({ directory: true, multiple: false, title: 'Choose a project directory' });
    const path = Array.isArray(result) ? result[0] : result;
    if (!path) return;
    beginPendingAction();
    try {
      const project = await api.addProject(path, 'pi');
      const existing = projects.find(candidate => candidate.path === project.path);
      if (!existing) projects = [...projects, project];
      if (projectToken === projectSelectionToken && threadToken === threadSelectionToken && !invalidatedProjects.has(project.id)) await selectProject(existing ?? project);
    } catch (error) {
      if (projectToken === projectSelectionToken && threadToken === threadSelectionToken) startupError = `Could not add project: ${errorText(error)}`;
    } finally { endPendingAction(); }
  }
  async function removeProject(project: Project) {
    if (!window.confirm(`Remove “${project.displayName}” from πDesk?\n\nRepository files, Git history, and harness sessions will not be deleted.`)) return;
    try {
      await api.removeProject(project.id);
      invalidatedProjects.add(project.id);
      const removingActiveProject = activeProject?.id === project.id;
      if (removingActiveProject) {
        ++projectSelectionToken;
        ++threadSelectionToken;
        activeProject = null;
        activeThread = null;
        activeSession = null;
        loadingThread = false;
        rightPanel = null;
        diffPath = undefined;
      }
      clearProjectCache(project.id);
      projects = projects.filter(candidate => candidate.id !== project.id);
      recentThreads = recentThreads.filter(thread => thread.projectId !== project.id);
      if (removingActiveProject) goHome();
    } catch (error) { startupError = `Could not remove project: ${errorText(error)}`; }
  }
  async function createThread(project: Project) {
    if (!installation || installInProgress) return;
    const projectToken = projectSelectionToken;
    const threadToken = threadSelectionToken;
    beginPendingAction();
    try {
      // Every new thread in a Git project gets its own worktree.
      const isolated = project.isGit;
      const thread = await api.createThread(project.id, 'pi', isolated);
      if (!invalidatedProjects.has(project.id)) {
        threadsByProject[project.id] = [thread, ...(threadsByProject[project.id] ?? []).filter(candidate => candidate.id !== thread.id)];
      }
      if (activeProject?.id === project.id && projectToken === projectSelectionToken && threadToken === threadSelectionToken) await selectThread(thread);
    } catch (error) {
      if (projectToken === projectSelectionToken && threadToken === threadSelectionToken && !invalidatedProjects.has(project.id)) startupError = `Could not start thread: ${errorText(error)}`;
    } finally { endPendingAction(); }
  }
  async function selectThread(thread: Thread) {
    if (!installation || installInProgress) return;
    expandProject(thread.projectId);
    const selectionToken = ++threadSelectionToken;
    const inflightStop = stopping.get(thread.id);
    if (inflightStop) {
      await inflightStop.catch(() => undefined);
      if (selectionToken !== threadSelectionToken || invalidatedProjects.has(thread.projectId) || invalidatedThreads.has(thread.id)) return;
    }
    leaveCurrentThread(thread.id);
    cancelIdleStop(thread.id);
    selectedThreadId = thread.id;
    threadMenu = null;
    activeThread = thread;
    loadingThread = true;
    rightPanel = null;
    localStorage.setItem('lastThread', thread.id);
    try {
      const cached = liveSessions.get(thread.id);
      if (cached) {
        cacheSession(thread.id, thread.projectId, cached);
        if (selectionToken === threadSelectionToken && activeThread?.id === thread.id) activeSession = cached;
      } else {
        const optimistic = optimisticSession(thread);
        if (optimistic) {
          // Show the empty thread now; Pi finishes starting behind it.
          activeSession = optimistic;
          loadingThread = false;
        }
        const opened = await openSessionModel(thread, optimistic ?? undefined);
        if (selectionToken !== threadSelectionToken || activeProject?.id !== thread.projectId || activeThread?.id !== thread.id || invalidatedProjects.has(thread.projectId) || invalidatedThreads.has(thread.id)) return;
        activeThread = opened.thread;
        activeSession = opened.model;
      }
      const viewedAt = new Date().toISOString();
      thread.lastViewedAt = viewedAt;
      const row = threadsByProject[thread.projectId]?.find(candidate => candidate.id === thread.id);
      if (row) row.lastViewedAt = viewedAt;
    } catch (error) {
      if (selectionToken === threadSelectionToken && activeProject?.id === thread.projectId && !invalidatedProjects.has(thread.projectId)) {
        startupError = `Could not open thread: ${errorText(error)}`;
        activeSession = null;
      }
    } finally {
      if (selectionToken === threadSelectionToken) loadingThread = false;
    }
  }
  async function openPanel(panel: 'changes') {
    const threadId = activeThread?.id;
    const projectId = activeThread?.projectId;
    const selectionToken = threadSelectionToken;
    rightPanel = panel;
    try {
      if (!ChangesPanel) {
        const component = (await import('$lib/components/diff/ChangesPanel.svelte')).default;
        if (activeThread?.id === threadId && activeProject?.id === projectId && rightPanel === panel && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId ?? '')) ChangesPanel = component;
      }
    } catch (error) {
      if (activeThread?.id === threadId && rightPanel === panel && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId ?? '')) {
        rightPanel = null;
        startupError = `Could not open ${panel}: ${errorText(error)}`;
      }
    }
  }
  function togglePanel(panel: 'changes') {
    if (rightPanel === panel) rightPanel = null;
    else void openPanel(panel);
  }
  /** Feedback is steered into the running turn before the plan is answered. */
  async function answerPlan(choice: 'approve' | 'decline' | 'feedback', feedback?: string) {
    const thread = activeThread;
    const request = planRequest;
    if (!thread || !request) return;
    try {
      if (choice === 'feedback' && feedback) await api.sendPrompt(thread.id, feedback, 'steer');
      await respond(request.id, { value: choice === 'approve' ? PLAN_APPROVE : choice === 'feedback' ? PLAN_FEEDBACK : PLAN_DECLINE });
    } catch (error) { startupError = `Could not answer the plan: ${errorText(error)}`; }
  }
  function showChanges(path?: string) {
    if (path && activeThread) {
      const cwd = activeThread.cwd.replace(/\/+$/, '');
      path = path.startsWith(`${cwd}/`) ? path.slice(cwd.length + 1) : path.replace(/^\.\//, '');
    }
    diffPath = path;
    void openPanel('changes');
  }
  /**
   * Pi's built-in terminal commands don't exist over RPC (sent as a prompt
   * they'd reach the model as text), so πDesk runs them. Returns false for
   * everything else, which goes to Pi: extension commands, skills, prompts.
   */
  async function runBuiltin(message: string): Promise<boolean> {
    const slash = parseSlash(message);
    const thread = activeThread;
    const session = activeSession;
    if (!slash || !thread || !session) return false;
    const { name, args } = slash;
    const say = (text: string, level: 'info' | 'warn' | 'error' = 'info') => session.notify(level, text);
    if (TERMINAL_ONLY.has(name)) {
      say(`/${name} only exists in Pi's own terminal UI. Open the Pi terminal, run pi, then /${name}.`, 'warn');
      return true;
    }
    switch (name) {
      case 'model': {
        if (!args) { document.querySelector<HTMLButtonElement>('button[aria-label="Select model"]')?.click(); return true; }
        const want = args.toLowerCase();
        const models = session.view.models;
        const match = models.find(model => modelKey(model).toLowerCase() === want)
          ?? models.find(model => model.id.toLowerCase() === want)
          ?? models.find(model => model.id.toLowerCase().includes(want) || model.name.toLowerCase().includes(want));
        if (!match) { say(`No model matches “${args}”. Type /model to choose from the list.`, 'warn'); return true; }
        await setModel(modelKey(match));
        say(`Switched to ${match.name} (${match.provider}).`);
        return true;
      }
      case 'thinking': {
        const levels = session.view.levels;
        if (!args) { say(`Effort is ${session.view.effort ?? 'the default'}. Choose one of: ${levels.join(', ') || 'none offered by this model'}.`); return true; }
        if (!levels.includes(args.toLowerCase())) { say(`“${args}” isn't an effort level here. Choose one of: ${levels.join(', ')}.`, 'warn'); return true; }
        await setEffort(args.toLowerCase());
        say(`Effort set to ${args.toLowerCase()}.`);
        return true;
      }
      case 'compact':
        say('Compacting the conversation…');
        try { await api.compactThread(thread.id, args || undefined); say('Compacted: older context is now a summary.'); }
        catch (error) { say(`Could not compact: ${errorText(error)}`, 'error'); }
        return true;
      case 'new':
        if (activeProject) await createThread(activeProject);
        return true;
      case 'name':
        if (!args) { say('Give the new name, for example /name Fix login bug.', 'warn'); return true; }
        await renameThread(thread, args.split('\n')[0].slice(0, 120));
        say(`Renamed to “${args.split('\n')[0].slice(0, 120)}”.`);
        return true;
      case 'plan':
      case 'auto':
        await setMode(name);
        say(name === 'plan' ? 'Plan mode: read-only until you approve a plan.' : 'Auto mode: full tools.');
        return true;
      case 'session': {
        try {
          const usage = await api.getUsage(thread.id);
          const context = usage.contextUsage?.percent != null ? ` · context ${Math.round(usage.contextUsage.percent)}% full` : '';
          say(`${usage.tokens.total.toLocaleString()} tokens (${usage.tokens.input.toLocaleString()} in, ${usage.tokens.output.toLocaleString()} out) · $${usage.cost.toFixed(4)}${context}`);
        } catch (error) { say(`Could not read session stats: ${errorText(error)}`, 'error'); }
        return true;
      }
      case 'copy': {
        const last = [...session.view.items].reverse().find(item => item.kind === 'text' && item.text.trim());
        if (!last || last.kind !== 'text') { say('There is no reply to copy yet.', 'warn'); return true; }
        try { await navigator.clipboard.writeText(last.text); say('Copied the last reply.'); }
        catch { say('Could not reach the clipboard.', 'error'); }
        return true;
      }
      case 'reload':
        say("Restarting this thread's Pi to reload extensions and settings…");
        await restart();
        return true;
      case 'login':
        openTerminal('pi');
        say('In the Pi terminal, type /login and pick your provider.');
        return true;
      default:
        return false;
    }
  }

  async function send(message: string, mode: 'prompt' | 'steer' | 'follow_up') {
    if (await runBuiltin(message)) return;
    const targetThread = activeThread;
    if (!targetThread) return;
    const projectId = targetThread.projectId;
    const selectionToken = threadSelectionToken;
    try {
      await api.sendPrompt(targetThread.id, message, mode);
      if (activeThread?.id === targetThread.id && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId)) startupError = '';
      if (!message.trim().startsWith('/') && !invalidatedProjects.has(projectId) && !invalidatedThreads.has(targetThread.id) && (!targetThread.title || targetThread.title === 'New thread')) {
        const title = message.trim().split('\n')[0].slice(0, 70);
        if (title) await renameThread(targetThread, title, selectionToken);
      }
    } catch (error) {
      if (activeThread?.id === targetThread.id && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId)) startupError = `Could not send message: ${errorText(error)}`;
      throw error;
    }
  }
  async function stop() {
    const targetThread = activeThread;
    if (!targetThread) return;
    const projectId = targetThread.projectId;
    const selectionToken = threadSelectionToken;
    try { await api.abortThread(targetThread.id); }
    catch (error) { if (activeThread?.id === targetThread.id && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId)) startupError = `Could not stop operation: ${errorText(error)}`; }
  }
  async function respond(requestId: string, response: UiResponse) {
    const targetThread = activeThread;
    const targetSession = activeSession;
    if (!targetThread) return;
    const projectId = targetThread.projectId;
    const selectionToken = threadSelectionToken;
    try {
      await api.respondUi(targetThread.id, requestId, response);
      if (invalidatedProjects.has(projectId) || invalidatedThreads.has(targetThread.id)) return;
      targetSession?.dismissRequest(requestId);
      updateThreadStatus(targetThread.id, 'active');
    } catch (error) {
      if (activeThread?.id === targetThread.id && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId)) startupError = `Could not respond: ${errorText(error)}`;
      throw error;
    }
  }
  async function restart() {
    const targetThread = activeThread;
    const targetSession = activeSession;
    if (!targetThread) return;
    const projectId = targetThread.projectId;
    const selectionToken = threadSelectionToken;
    try {
      const snapshot = await api.restartThread(targetThread.id);
      if (invalidatedProjects.has(projectId) || invalidatedThreads.has(targetThread.id)) return;
      const existing = liveSessions.get(targetThread.id) ?? targetSession;
      const model = existing ?? new SessionModel(snapshot);
      model.reconnect(snapshot);
      cacheSession(targetThread.id, snapshot.thread.projectId, model);
      Object.assign(targetThread, snapshot.thread);
      const rows = threadsByProject[snapshot.thread.projectId];
      const row = rows?.find(candidate => candidate.id === snapshot.thread.id);
      if (row) Object.assign(row, snapshot.thread);
      if (activeThread?.id === targetThread.id && activeSession === model && threadSelectionToken === selectionToken) {
        activeThread = snapshot.thread;
        activeSession = model;
      }
      delete crashDetails[targetThread.id];
    } catch (error) {
      if (activeThread?.id === targetThread.id && activeSession === targetSession && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId)) startupError = `Could not restart session: ${errorText(error)}`;
    }
  }
  async function renameThread(thread: Thread, title: string, expectedSelectionToken?: number) {
    const clean = title.trim();
    if (expectedSelectionToken === undefined) renaming = null;
    if (!clean) return;
    const projectId = thread.projectId;
    try {
      const updated = await api.renameThread(thread.id, clean);
      if (invalidatedProjects.has(projectId) || invalidatedThreads.has(thread.id)) return;
      Object.assign(thread, updated);
      const rows = threadsByProject[updated.projectId];
      const row = rows?.find(candidate => candidate.id === updated.id);
      if (row) {
        Object.assign(row, updated);
        threadsByProject[updated.projectId] = [...rows];
      }
      if (activeThread?.id === updated.id) Object.assign(activeThread, updated);
    } catch (error) {
      if (!invalidatedProjects.has(projectId) && (expectedSelectionToken === undefined || expectedSelectionToken === threadSelectionToken)) startupError = errorText(error);
    }
  }
  function startRename(thread: Thread) {
    threadMenu = null;
    renaming = thread.id;
    renameText = thread.title;
  }
  function showThreadMenu(thread: Thread, x: number, y: number) {
    projectMenu = null;
    renaming = null;
    threadMenu = threadMenu?.id === thread.id ? null : {
      id: thread.id,
      x: Math.max(8, Math.min(innerWidth - 212, x)),
      y: Math.max(8, Math.min(innerHeight - 178, y)),
    };
  }
  async function previewDelete(thread: Thread) {
    threadMenu = null;
    if (previewingDelete) return;
    previewingDelete = true;
    try {
      const preview = await api.threadDeletePreview(thread.id);
      if (!invalidatedThreads.has(thread.id) && threadsByProject[thread.projectId]?.some(row => row.id === thread.id)) deleteDialog = { thread, preview, busy: false };
    } catch (error) { startupError = `Could not inspect thread: ${errorText(error)}`; }
    finally { previewingDelete = false; }
  }
  async function confirmDelete() {
    const dialog = deleteDialog;
    if (!dialog || dialog.busy) return;
    dialog.busy = true;
    const thread = dialog.thread;
    try {
      await api.deleteThread(thread.id, dialog.preview.changedFiles > 0);
      addInvalidatedThread(thread.id);
      cancelIdleStop(thread.id);
      prewarmedAt.delete(thread.id);
      const pending = openingSessions.get(thread.id);
      if (pending) {
        pending.discarded = true;
        pending.reject?.(new Error('Thread was deleted.'));
      }
      removeCachedSession(thread.id);
      delete crashDetails[thread.id];
      const visible = visibleThreads(thread.projectId);
      const index = visible.findIndex(row => row.id === thread.id);
      threadsByProject[thread.projectId] = (threadsByProject[thread.projectId] ?? []).filter(row => row.id !== thread.id);
      if (localStorage.getItem('lastThread') === thread.id) localStorage.removeItem('lastThread');
      deleteDialog = null;
      if (activeThread?.id === thread.id) {
        ++threadSelectionToken;
        activeThread = null;
        selectedThreadId = null;
        activeSession = null;
        loadingThread = false;
        rightPanel = null;
        const next = visible[index + 1] ?? visible[index - 1];
        if (next) void selectThread(next);
      }
    } catch (error) {
      startupError = `Could not delete thread: ${errorText(error)}`;
      dialog.busy = false;
    }
  }
  async function setFlag(thread: Thread, kind: 'pinned' | 'archived') {
    const projectId = thread.projectId;
    const projectToken = projectSelectionToken;
    const threadToken = threadSelectionToken;
    try {
      const updated = await api.setThreadFlags(thread.id, kind === 'pinned' ? !thread.pinned : undefined, kind === 'archived' ? !thread.archived : undefined);
      if (invalidatedProjects.has(projectId) || invalidatedThreads.has(thread.id)) return;
      Object.assign(thread, updated);
    } catch (error) {
      if (projectToken === projectSelectionToken && threadToken === threadSelectionToken && !invalidatedProjects.has(projectId)) startupError = errorText(error);
    }
  }
  async function setMode(mode: ThreadMode) {
    const thread = activeThread;
    if (!thread || thread.mode === mode) return;
    const projectId = thread.projectId;
    try {
      const updated = await api.setThreadMode(thread.id, mode);
      if (invalidatedProjects.has(projectId) || invalidatedThreads.has(thread.id)) return;
      const row = threadsByProject[projectId]?.find(candidate => candidate.id === thread.id);
      if (row) row.mode = updated.mode;
      if (activeThread?.id === thread.id) activeThread.mode = updated.mode;
    } catch (error) { startupError = `Could not change mode: ${errorText(error)}`; }
  }
  async function setModel(value: string) {
    const targetThread = activeThread;
    const targetSession = activeSession;
    if (!targetThread) return;
    const projectId = targetThread.projectId;
    const selectionToken = threadSelectionToken;
    const slash = value.indexOf('/'); if (slash < 0) return;
    try {
      const state = await api.setThreadModel(targetThread.id, value.slice(0, slash), value.slice(slash + 1));
      if (activeThread?.id === targetThread.id && activeSession === targetSession && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId) && !invalidatedThreads.has(targetThread.id)) {
        if (state.model && targetSession) targetSession.view.model = state.model;
        if (targetSession) targetSession.view.effort = state.thinkingLevel;
      }
    } catch (error) {
      if (activeThread?.id === targetThread.id && activeSession === targetSession && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId)) startupError = `Could not change model: ${errorText(error)}`;
    }
  }
  async function setEffort(level: string) {
    const targetThread = activeThread;
    const targetSession = activeSession;
    if (!targetThread) return;
    const projectId = targetThread.projectId;
    const selectionToken = threadSelectionToken;
    try {
      const state = await api.setThreadEffort(targetThread.id, level);
      if (activeThread?.id === targetThread.id && activeSession === targetSession && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId) && !invalidatedThreads.has(targetThread.id) && targetSession) targetSession.view.effort = state.thinkingLevel;
    } catch (error) {
      if (activeThread?.id === targetThread.id && activeSession === targetSession && threadSelectionToken === selectionToken && !invalidatedProjects.has(projectId)) startupError = `Could not change effort: ${errorText(error)}`;
    }
  }
  async function install() {
    if (installInProgress || !installReady || checkingRuntime) return;
    installInProgress = true;
    installerVisible = true;
    installStatus = 'preparing';
    installLog = [];
    installError = '';
    try {
      await api.installHarness('pi');
      if (installError) throw new Error(installError);
      await refreshHarnesses();
      if (!installation) throw new Error('Installation finished, but the private Pi could not be verified. Review the output and retry.');
      installStatus = 'complete';
      installError = '';
    } catch (error) {
      installError = errorText(error);
      appendInstallLog(installError);
      installStatus = 'failed';
    } finally { installInProgress = false; }
  }
  function openSettings(section?: SettingsSection) {
    if (section) settingsSection = section;
    else if (extensionUpdates) settingsSection = 'extensions';
    settingsOpen = true;
  }
  async function checkExtensionUpdates() {
    try { extensionUpdates = (await checkForUpdates()).filter(item => item.updateAvailable).length; }
    catch { /* Offline or no packages: the badge simply stays off. */ }
  }
  /** Optional `run` is typed into the new shell, e.g. an MCP sign-in. */
  let terminalRun = $state<string | undefined>();
  function openTerminal(run?: unknown) {
    if (!installation) return;
    // Also used directly as a click handler, which passes the event.
    terminalRun = typeof run === 'string' ? run : undefined;
    terminalOpen = true;
  }
  function toggleIntern() { internStarted = true; internOpen = !internOpen; }
  async function refreshInternState() {
    const epoch = ++internRefreshEpoch;
    const project = activeProject;
    try {
      const nextProjects = await api.listProjects();
      const rows = project && nextProjects.some(row => row.id === project.id) ? await api.listThreads(project.id) : null;
      if (epoch !== internRefreshEpoch) return;
      for (const previous of projects) {
        if (!nextProjects.some(row => row.id === previous.id)) { invalidatedProjects.add(previous.id); clearProjectCache(previous.id); }
      }
      projects = nextProjects;
      if (activeProject?.id !== project?.id) return;
      if (project && !rows) {
        ++projectSelectionToken; ++threadSelectionToken;
        activeProject = null; activeThread = null; activeSession = null; selectedThreadId = null; rightPanel = null; loadingThread = false;
        localStorage.removeItem('lastProject'); localStorage.removeItem('lastThread');
      } else if (project && rows) {
        for (const previous of threadsByProject[project.id] ?? []) {
          if (!rows.some(row => row.id === previous.id)) { addInvalidatedThread(previous.id); cancelIdleStop(previous.id); removeCachedSession(previous.id); delete crashDetails[previous.id]; }
        }
        threadsByProject[project.id] = rows;
        if (activeThread) {
          const updated = rows.find(row => row.id === activeThread?.id);
          if (updated) activeThread = updated;
          else { ++threadSelectionToken; activeThread = null; activeSession = null; selectedThreadId = null; rightPanel = null; loadingThread = false; localStorage.removeItem('lastThread'); }
        }
      }
    } catch (error) { if (epoch === internRefreshEpoch) startupError = `Could not refresh Intern changes: ${errorText(error)}`; }
  }
  async function copyInstallPath() {
    if (!installation) return;
    try {
      await navigator.clipboard.writeText(installation.path);
      copiedPath = true;
    } catch { startupError = 'Could not copy path. Select it in Settings instead.'; }
  }
  async function revealInstallPath() {
    if (!installation) return;
    try { await revealItemInDir(installation.path); }
    catch { startupError = 'Could not reveal the private Pi installation in Finder.'; }
  }
  async function openSwitcher() {
    switchQuery = ''; switchIndex = 0; switcherOpen = true;
    await Promise.all(projects.map(p => refreshThreads(p.id)));
    await tick();
    if (switcherOpen) document.querySelector<HTMLInputElement>('#switcher-search')?.focus();
  }
  async function chooseSwitch(entry: (typeof switchEntries)[number]) {
    switcherOpen = false;
    if (entry.kind === 'terminal') { openTerminal(); }
    else if (entry.kind === 'project') {
      await selectProject(entry.project);
    } else if (activeProject?.id === entry.project.id) {
      await selectThread(entry.thread);
    } else {
      const selectionToken = beginProjectSelection(entry.project);
      void refreshThreads(entry.project.id, selectionToken);
      await selectThread(entry.thread);
    }
  }
  const STATUS_LABEL: Record<ThreadStatus, string> = {
    active: 'Working', waiting: 'Needs input', idle: 'Idle', completed: 'Done', failed: 'Failed', disconnected: 'Disconnected',
  };
  function setTheme(next: typeof theme) { theme = next; applyTheme(); }
  function onWindowClick(event: MouseEvent) {
    if (projectMenu && !(event.target as Element | null)?.closest('.project-menu, .row-more')) projectMenu = null;
    if (threadMenu && !(event.target as Element | null)?.closest('.thread-menu, .thread-more')) threadMenu = null;
  }
  function focusInput(node: HTMLInputElement) { node.focus(); node.select(); }
</script>

<svelte:window onclick={onWindowClick} />

<svelte:boundary>
<div class="app-shell" style={`--sidebar-width:${sidebarWidth}px; --panel-width:${panelWidth}px`}>
  <aside class="sidebar" aria-label="Projects and threads">
    <div class="sidebar-top" data-tauri-drag-region>
      <div class="sidebar-tools">
        <button class="icon-button" class:pressed={!activeProject} title="Welcome" aria-label="Welcome" onclick={goHome}><House size={16} strokeWidth={1.8} /></button>
        <button class="icon-button" title="Search projects and threads (⌘K)" aria-label="Switch project or thread" onclick={() => void openSwitcher()}><Search size={16} strokeWidth={1.8} /></button>
        <button class="icon-button" title="New thread (⌘N)" aria-label="New thread" disabled={!activeProject || pendingAction || !harnesses.length} onclick={() => { if (activeProject) void createThread(activeProject); }}><SquarePen size={16} strokeWidth={1.8} /></button>
      </div>
    </div>

    <div class="project-list">
      <div class="section-label"><span>Projects</span><button class="mini-button" title="Add project" aria-label="Add project" onclick={() => void addProject()}><Plus size={14} strokeWidth={2} /></button></div>
      {#each projects as project (project.id)}
        {@const selected = activeProject?.id === project.id}
        {@const open = selected && !collapsedProjects.has(project.id)}
        <section class="project-section">
          <div class="project-row" class:active={selected}>
            <button class="project-toggle" onclick={() => toggleProject(project)} aria-label={`Open ${project.displayName}`} aria-expanded={open} title={selected ? (open ? 'Collapse project' : 'Expand project') : `Open ${project.displayName}`}>
              <ChevronRight size={12} strokeWidth={2.2} class={open ? 'chev open' : 'chev'} />
              <span class="project-glyph" aria-hidden="true">{project.displayName[0]?.toUpperCase()}</span>
              <span class="project-name">{project.displayName}</span>
            </button>
            <button class="row-more" title="Project actions" aria-label={`Actions for ${project.displayName}`} aria-haspopup="menu" aria-expanded={projectMenu === project.id} onclick={() => projectMenu = projectMenu === project.id ? null : project.id}><MoreHorizontal size={14} /></button>
            {#if projectMenu === project.id}
              <div class="project-menu" role="menu">
                <div class="menu-caption" title={project.path}>{project.path}</div>
                <button role="menuitem" class="danger" onclick={() => { projectMenu = null; void removeProject(project); }}><Trash2 size={13} /> Remove from app</button>
              </div>
            {/if}
          </div>
          {#if open}
            {@const visible = visibleThreads(project.id)}
            <div class="thread-list">
              <div class="new-thread-row">
                <button class="new-thread" disabled={pendingAction || !harnesses.length} onclick={() => void createThread(project)}><Plus size={13} strokeWidth={2.2} /> New thread</button>
              </div>
              {#if visible.length}
                <VList data={visible} getKey={thread => thread.id} style={`height: min(58vh, ${visible.length * 46}px);`}>
                  {#snippet children(thread)}
                    <div class:active={selectedThreadId === thread.id} class="thread-row" oncontextmenu={event => { event.preventDefault(); threadMenu = null; showThreadMenu(thread, event.clientX, event.clientY); }} role="group" aria-label={`${thread.title || 'New thread'} thread`}>
                      {#if renaming === thread.id}
                        <span class={`status-dot ${thread.status}`} role="img" aria-label={thread.status}></span>
                        <input class="thread-rename" use:focusInput aria-label="Thread title" bind:value={renameText} maxlength="120"
                          onkeydown={event => {
                            if (event.key === 'Enter') { event.preventDefault(); event.currentTarget.blur(); }
                            if (event.key === 'Escape') { event.stopPropagation(); renaming = null; }
                          }}
                          onblur={() => { if (renaming === thread.id) void renameThread(thread, renameText); }} />
                      {:else}
                        {@const waiting = waitingLabel(thread)}
                        {@const stats = rowStats[thread.id]}
                        <button class="thread-link" onclick={() => { cancelPrewarm(); void selectThread(thread); }} ondblclick={() => startRename(thread)} onpointerenter={() => prewarmSoon(thread)} onpointerleave={cancelPrewarm} onfocus={() => prewarmSoon(thread)} onblur={cancelPrewarm} title={thread.title}>
                          <span class={`status-dot ${thread.status}`} role="img" aria-label={thread.status}></span>
                          <span class="thread-text">
                            <span class="thread-line">
                              <span class="thread-title">{thread.title || 'New thread'}</span>
                              {#if thread.pinned}<Pin size={10} strokeWidth={2.2} class="pin-mark" aria-label="Pinned" />{/if}
                              {#if waiting}{#key waiting}<span class="row-pill" class:plan={waiting === 'Needs approval'}>{waiting}</span>{/key}{/if}
                            </span>
                            <span class="thread-meta">
                              {#if thread.status === 'active'}<span class="meta-working">Working {clock(now - (activeSince[thread.id] ?? now))}</span>
                              {:else if thread.status === 'failed'}<span class="meta-bad">Failed</span>
                              {:else if thread.status === 'disconnected'}<span>Disconnected</span>
                              {:else if thread.status === 'completed'}<span class="meta-done">Done</span>
                              {:else if !waiting}<span>{ago(thread.lastViewedAt, now)}</span>{/if}
                              {#if thread.mode === 'plan'}<span class="meta-chip">Plan</span>{/if}
                              {#if thread.worktreePath}<span class="meta-chip mono" title={thread.worktreePath}>{stats?.branch ?? 'worktree'}</span>{/if}
                              {#if stats && (stats.additions || stats.deletions)}<span class="meta-diff"><span class="add"><CountUp value={stats.additions} prefix="+" /></span> <span class="del"><CountUp value={stats.deletions} prefix="−" /></span></span>{/if}
                            </span>
                          </span>
                        </button>
                      {/if}
                      <button class="thread-more" title={`Actions for ${thread.title}`} aria-label={`Actions for ${thread.title}`} aria-haspopup="menu" aria-expanded={threadMenu?.id === thread.id} onclick={event => { const rect = event.currentTarget.getBoundingClientRect(); showThreadMenu(thread, rect.right - 200, rect.bottom + 4); }}><MoreHorizontal size={14} /></button>
                    </div>
                  {/snippet}
                </VList>
              {:else}
                <div class="threads-empty">No threads yet</div>
              {/if}
            </div>
          {/if}
        </section>
      {/each}
      {#if !projects.length && !detecting}<div class="empty-projects">Add a folder to keep agent sessions next to the code they change.</div>{/if}
    </div>

    <div class="sidebar-bottom">
      <button class="footer-button" disabled={pendingAction} onclick={() => void addProject()}><FolderPlus size={15} strokeWidth={1.8} /> Add project</button>
      <span class="grow"></span>
      {#if projects.length}<button class="icon-button" class:pressed={showArchived} title={showArchived ? 'Hide archived threads' : 'Show archived threads'} aria-label={showArchived ? 'Hide archived' : 'Show archived'} aria-pressed={showArchived} onclick={() => showArchived = !showArchived}><Archive size={15} strokeWidth={1.8}/></button>{/if}
      <button class="icon-button settings-button" title={extensionUpdates ? `Settings · ${extensionUpdates} extension update${extensionUpdates === 1 ? '' : 's'}` : 'Settings (⌘,)'} aria-label="Settings" onclick={() => void openSettings()}><Settings2 size={15} strokeWidth={1.8} />{#if extensionUpdates}<span class="gear-badge" aria-hidden="true"></span>{/if}</button>
    </div>
  </aside>
  {#if threadMenu}
    {@const menuThread = (threadsByProject[activeProject?.id ?? ''] ?? []).find(row => row.id === threadMenu?.id) ?? Object.values(threadsByProject).flat().find(row => row.id === threadMenu?.id)}
    {#if menuThread}
      <div class="project-menu thread-menu" role="menu" style={`left:${threadMenu.x}px;top:${threadMenu.y}px`}>
        <button role="menuitem" onclick={() => startRename(menuThread)}><SquarePen size={13}/> Rename</button>
        <button role="menuitem" onclick={() => { const target = menuThread; threadMenu = null; void setFlag(target, 'pinned'); }}><Pin size={13}/> {menuThread.pinned ? 'Unpin' : 'Pin'}</button>
        <button role="menuitem" onclick={() => { const target = menuThread; threadMenu = null; void setFlag(target, 'archived'); }}><Archive size={13}/> {menuThread.archived ? 'Unarchive' : 'Archive'}</button>
        <div class="menu-divider"></div>
        <button role="menuitem" class="danger" onclick={() => void previewDelete(menuThread)}><Trash2 size={13}/> Delete thread…</button>
      </div>
    {/if}
  {/if}
  <div class="resize-handle" role="slider" tabindex="0" aria-orientation="vertical" aria-valuemin="205" aria-valuemax="390" aria-valuenow={sidebarWidth} aria-label="Resize project sidebar" onmousedown={() => sidebarResizing = true} onkeydown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); sidebarWidth = Math.max(205, Math.min(390, sidebarWidth + (event.key === 'ArrowRight' ? 12 : -12))); localStorage.setItem('sidebarWidth', String(sidebarWidth)); } }}></div>

  <main class="main-pane">
    <header class="main-header" data-tauri-drag-region>
      <div class="crumbs" data-tauri-drag-region>
        {#if activeProject}<span class="crumb-project">{activeProject.displayName}</span>{:else}<span class="crumb-project">πDesk</span>{/if}
        {#if visibleThread}<ChevronRight size={13} strokeWidth={2} class="crumb-sep" /><span class="top-thread">{visibleThread.title || 'New thread'}</span>{/if}
        {#if visibleThread?.worktreePath}
          {@const worktree = visibleThread.worktreePath}
          <button class="worktree-chip" title={`Worktree: ${worktree}\nClick to show in Finder`} aria-label={`Worktree ${worktreeLabel(visibleThread)}, show in Finder`} onclick={() => void revealWorktree(worktree)}>
            <GitFork size={12} strokeWidth={2} /><span>{worktreeLabel(visibleThread)}</span>
          </button>
        {/if}
        {#if visibleThread && currentView && currentView.status !== 'idle'}<span class={`status-pill ${currentView.status}`} title={STATUS_LABEL[currentView.status]}><span class={`status-dot ${currentView.status}`}></span><span class="pill-label">{currentView.status === 'waiting' ? (planRequest ? 'Needs approval' : STATUS_LABEL.waiting) : STATUS_LABEL[currentView.status]}{#if currentView.status === 'active' && activeSince[visibleThread.id]}<span class="pill-clock">{clock(now - activeSince[visibleThread.id])}</span>{/if}</span></span>{/if}
      </div>
      <div class="header-actions">
        <button class="toggle-button" class:pressed={internOpen} aria-label="Ask Pi Intern" aria-expanded={internOpen} title="Ask Pi Intern" onclick={toggleIntern}><Bot size={16}/><span>Intern{internPending ? ` (${internPending})` : ''}</span></button>
        {#if visibleThread && currentView}
          <div class="panel-toggles">
            <button class:pressed={rightPanel === 'changes'} class="toggle-button" title="Changes (⌘⇧D)" aria-label="Toggle changes" aria-pressed={rightPanel === 'changes'} onclick={() => togglePanel('changes')}><GitCompareArrows size={14} strokeWidth={1.9} /><span>Changes</span></button>
          </div>
        {/if}
      </div>
    </header>

    {#if startupError}<div class="error-banner" role="alert"><AlertTriangle size={14}/><span>{startupError}</span><button aria-label="Dismiss error" onclick={() => startupError = ''}><X size={14}/></button></div>{/if}
    {#if detecting}<div class="main-empty"><LoaderCircle class="spin" size={24} strokeWidth={1.6}/><h2>Checking private Pi</h2><p>Looking only inside πDesk’s installation directory.</p></div>
    {:else if onboarding || installerVisible}
      <PiSetup plan={installPlan} status={installStatus} busy={installInProgress} lines={installLog} error={installError || installPlanError} ready={installReady} checking={checkingRuntime} onInstall={() => void install()} onCheck={() => void checkPrivateRuntime()} onContinue={() => { installerVisible = false; installStatus = 'idle'; }} onOpenTerminal={openTerminal} />
    {:else if !activeProject}
      <Welcome {projects} recent={recentThreads} busy={pendingAction} onOpenProject={project => void selectProject(project)} onOpenThread={thread => void openThreadById(thread.id, thread.projectId)} onAddProject={() => void addProject()} onOpenIntern={() => { internStarted = true; internOpen = true; }} />
    {:else if loadingThread}
      <div class="main-empty delayed"><LoaderCircle class="spin" size={24} strokeWidth={1.6}/><h2>Opening thread</h2><p>Restoring the conversation from Pi.</p></div>
    {:else if !visibleThread || !currentView}
      <div class="main-empty"><div class="empty-graphic"><SquarePen size={24} strokeWidth={1.5}/></div><h2>What shall we work on?</h2><p>Start a thread in <strong>{activeProject.displayName}</strong> to talk to your agent.</p><button class="primary-button" disabled={pendingAction || !harnesses.length} onclick={() => void createThread(activeProject!)}><Plus size={15} strokeWidth={2.2}/> New thread</button><div class="empty-hint"><kbd>⌘</kbd><kbd>N</kbd> new thread <span class="dot-sep"></span> <kbd>⌘</kbd><kbd>K</kbd> jump anywhere</div></div>
    {:else}
      {#if currentView.error}<div class="error-banner"><AlertTriangle size={14}/><span>{currentView.error}</span>{#if visibleThread && crashDetails[visibleThread.id]}<button onclick={() => errorDetailsOpen = true}>View details</button>{/if}<button class="banner-action" onclick={() => void restart()}><RefreshCw size={13}/> Restart session</button></div>{/if}
      <Conversation view={currentView} onSend={send} onAbort={stop} onShowChanges={showChanges} onRespond={respond} onSetModel={setModel} onSetEffort={setEffort} {defaultModelKey} onMakeDefault={makeDefaultModel} agentMode={visibleThread.mode ?? 'auto'} onSetMode={setMode} plan={planRequest ? { plan: planOf(planRequest) ?? '', onApprove: () => answerPlan('approve'), onDecline: () => answerPlan('decline'), onFeedback: text => answerPlan('feedback', text), onReview: () => rightPanel = 'plan' } : undefined} />
    {/if}
    {#if !detecting && !onboarding && !installerVisible}
      <footer class="status-bar">
        <RuntimeMonitor {projects} activeThreadId={visibleThread?.id ?? null} onStop={stopThreadsFromMonitor} onOpenThread={(threadId, projectId) => void openThreadById(threadId, projectId)} />
      </footer>
    {/if}
  </main>

  {#if rightPanel && visibleThread && currentView}
    <div class="resize-handle panel-handle" role="slider" tabindex="0" aria-orientation="vertical" aria-valuemin="320" aria-valuemax="850" aria-valuenow={panelWidth} aria-label="Resize detail panel" onmousedown={() => panelResizing = true} onkeydown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); panelWidth = Math.max(320, Math.min(850, panelWidth + (event.key === 'ArrowLeft' ? 12 : -12))); localStorage.setItem('panelWidth', String(panelWidth)); } }}></div>
    <aside class="details-pane" aria-label={rightPanel === 'plan' ? 'Plan' : 'Changes'}>
      {#if rightPanel === 'plan' && planRequest}<PlanReview plan={planOf(planRequest) ?? ''} source={visibleThread.title || 'New thread'} onApprove={() => answerPlan('approve')} onDecline={() => answerPlan('decline')} onFeedback={text => answerPlan('feedback', text)} onClose={() => rightPanel = null} />
      {:else if ChangesPanel}<ChangesPanel thread={visibleThread} onClose={() => rightPanel = null} focusPath={diffPath} />
      {:else}<div class="panel-loading"><LoaderCircle class="spin" size={16}/><span>Loading changes…</span></div>{/if}
    </aside>
  {/if}
</div>

  {#snippet failed(error, reset)}
    <div class="main-empty" role="alert" style="height: 100vh;">
      <div class="empty-graphic danger"><AlertTriangle size={26} strokeWidth={1.5} /></div>
      <h2>Something went wrong</h2>
      <p>{error instanceof Error ? error.message : String(error ?? 'An unexpected error occurred.')}</p>
      <button class="primary-button" onclick={() => { startupError = ''; reset(); }}><RefreshCw size={15} /> Try again</button>
      <div class="empty-hint">If this keeps happening, reload the window.</div>
    </div>
  {/snippet}

{#if deleteDialog}
  <ConfirmDialog title="Delete thread?" path={deleteDialog.thread.title || 'New thread'}
    detail={`Conversation history will be deleted.${deleteDialog.preview.worktreePath ? `\nIsolated worktree ${deleteDialog.preview.worktreePath} will be removed.` : ''}${deleteDialog.preview.changedFiles ? `\n${deleteDialog.preview.changedFiles} uncommitted files will be discarded.` : ''}`}
    confirmLabel={deleteDialog.preview.changedFiles ? 'Delete thread and discard changes' : 'Delete thread'}
    busy={deleteDialog.busy} onConfirm={() => void confirmDelete()} onCancel={() => deleteDialog = null} />
{/if}

{#if errorDetailsOpen && visibleThread}
  <div class="overlay" role="presentation" onclick={event => { if (event.target === event.currentTarget) errorDetailsOpen = false; }}>
    <div class="settings dialog" role="dialog" aria-modal="true" aria-label="Pi error details">
      <header><h2>Pi error</h2><button class="icon-button" aria-label="Close details" onclick={() => errorDetailsOpen = false}><X size={16}/></button></header>
      <pre class="error-details">{crashDetails[visibleThread.id]}</pre>
    </div>
  </div>
{/if}

{#if switcherOpen}
  <div class="overlay" role="presentation" onclick={event => { if (event.target === event.currentTarget) switcherOpen = false; }}>
    <div class="switcher dialog" role="dialog" aria-modal="true" aria-label="Switch project or thread">
      <div class="switch-search">
        <Search size={17} strokeWidth={1.8}/>
        <input id="switcher-search" placeholder="Jump to a project or thread…" bind:value={switchQuery}
          oninput={() => switchIndex = 0}
          onkeydown={event => {
            if (event.key === 'ArrowDown') { event.preventDefault(); switchIndex = Math.min(switchEntries.length - 1, switchIndex + 1); }
            if (event.key === 'ArrowUp') { event.preventDefault(); switchIndex = Math.max(0, switchIndex - 1); }
            if (event.key === 'Enter' && switchEntries[switchIndex]) { event.preventDefault(); void chooseSwitch(switchEntries[switchIndex]); }
          }} />
        <kbd>esc</kbd>
      </div>
      <div class="switch-results">
        {#each switchEntries as entry, index}
          <button class:selected={switchIndex === index} onmouseenter={() => switchIndex = index} onclick={() => void chooseSwitch(entry)}>
            <span class="switch-icon" class:thread={entry.kind === 'thread'}>{#if entry.kind === 'project'}<Folder size={14} strokeWidth={1.8}/>{:else if entry.kind === 'terminal'}<Terminal size={14} strokeWidth={1.8}/>{:else}<MessageSquare size={14} strokeWidth={1.8}/>{/if}</span>
            <span class="switch-text"><strong>{entry.label}</strong><small>{entry.subtitle}</small></span>
            {#if entry.kind === 'thread'}<span class={`status-dot ${entry.thread.status}`}></span>{/if}
          </button>
        {/each}
        {#if !switchEntries.length}<p>No matching projects or threads.</p>{/if}
      </div>
      <div class="dialog-footer"><span><kbd>↑</kbd><kbd>↓</kbd> navigate</span><span><kbd>↵</kbd> open</span></div>
    </div>
  </div>
{/if}

{#if settingsOpen}
  <div class="settings-page" role="dialog" aria-modal="true" aria-label="Settings">
    <nav class="settings-nav" aria-label="Settings sections" data-tauri-drag-region>
      <div class="settings-nav-title" data-tauri-drag-region>Settings</div>
      {#each SETTINGS_SECTIONS as section (section.id)}
        <button class="settings-nav-item" class:on={settingsSection === section.id} aria-current={settingsSection === section.id ? 'page' : undefined} onclick={() => settingsSection = section.id}>
          <section.icon size={15} strokeWidth={1.8} /><span>{section.label}</span>
          {#if section.id === 'extensions' && extensionUpdates}<span class="nav-badge" title={`${extensionUpdates} update${extensionUpdates === 1 ? '' : 's'} available`}>{extensionUpdates}</span>{/if}
        </button>
      {/each}
      <span class="grow"></span>
      <div class="settings-nav-foot">πDesk · Pi {installation?.version ?? 'not installed'}</div>
    </nav>
    <div class="settings-main">
      <header class="settings-top" data-tauri-drag-region>
        <button class="icon-button" aria-label="Close settings" title="Close (Esc)" onclick={() => settingsOpen = false}><X size={17}/></button>
      </header>
      <div class="settings-scroll">
        {#key settingsSection}
        <div class="settings-inner">
          {#if settingsSection === 'general'}
            <h1>General</h1><p class="lead">How πDesk looks, and what it keeps private.</p>
            <h2>Appearance</h2>
            <div class="settings-card">
              <div class="setting-row"><span>Theme</span>
          <div class="segmented" role="radiogroup" aria-label="Theme">
            <button role="radio" aria-checked={theme === 'system'} class:on={theme === 'system'} onclick={() => setTheme('system')}><Monitor size={13}/> System</button>
            <button role="radio" aria-checked={theme === 'light'} class:on={theme === 'light'} onclick={() => setTheme('light')}><Sun size={13}/> Light</button>
            <button role="radio" aria-checked={theme === 'dark'} class:on={theme === 'dark'} onclick={() => setTheme('dark')}><Moon size={13}/> Dark</button>
          </div>
        </div>
            </div>
            <h2>Privacy</h2>
            <div class="settings-card prose"><p class="last">Pi stores its settings, extensions, credentials, and sessions inside πDesk’s private directory. Nothing is copied from your terminal Pi, and it is never modified. πDesk sends no product telemetry.</p></div>
          {:else if settingsSection === 'models'}
            <h1>Models</h1>
            <p class="lead">Every new thread starts with this model and effort. Existing threads keep their own.</p>
        <div class="setting-group">
          <div class="setting-row"><span class="setting-name">Default model</span>
            {#if knownModels.length}
              <ModelPicker models={knownModels} current={knownModels.find(model => modelKey(model) === defaultModelKey) ?? null} defaultKey={defaultModelKey} placement="down" label="Default model for new threads"
                onSelect={key => { const model = knownModels.find(candidate => modelKey(candidate) === key); if (model) void makeDefaultModel(model); }} />
            {:else}
              <span class="missing">Open a thread once to load models</span>
            {/if}
          </div>
          <div class="setting-row"><span class="setting-name">Default effort</span>
            <select class="setting-select" aria-label="Default effort for new threads" value={modelDefaults?.thinkingLevel ?? ''} onchange={event => void setDefaultEffort(event.currentTarget.value)}>
              {#if !modelDefaults?.thinkingLevel}<option value="">Pi default (medium)</option>{/if}
              {#each THINKING_LEVELS as level (level)}<option value={level}>{level[0].toUpperCase() + level.slice(1)}</option>{/each}
            </select>
          </div>
        </div>
          {:else if settingsSection === 'pi'}
            <h1>Pi runtime</h1>
            <p class="lead">πDesk uses only its own copy under <code>~/.pidesk</code>. Your system Pi is never selected or modified.</p>
        <div class="setting-group">
          <div class="setting-row"><span class="setting-name"><Terminal size={15} strokeWidth={1.8}/> Pi</span>
            {#if installation}<span class="version">{installation.version}</span>
            {:else}<span class="missing">Not found</span>{/if}
            {#if !installation}<button class="secondary-button small" onclick={() => { settingsOpen = false; installerVisible = true; }}>Set up Pi</button>{/if}
          </div>
          {#if installation}
            <div class="install-paths">
              <span>Executable</span><code>{installation.path}</code>
              <span>Runtime</span><code>{installPlan?.installPath}</code>
              <span>Private data</span><code>{installPlan?.agentDir}</code>
            </div>
          {/if}
        </div>
        {#if installation}
          <div class="install-actions">
            <button class="secondary-button small" onclick={() => void copyInstallPath()}>{copiedPath ? 'Copied' : 'Copy path'}</button>
            <button class="secondary-button small" onclick={() => void revealInstallPath()}>Reveal in Finder</button>
            <button class="secondary-button small" onclick={openTerminal}><Terminal size={13}/> Open terminal</button>
          </div>
        {/if}
        <button class="text-button" disabled={installInProgress || checkingRuntime} onclick={() => void checkPrivateRuntime()}><RefreshCw size={13}/> Check private installation</button>
        {#if installPlanError}<p role="alert">{installPlanError}</p>{/if}
            {#if installation && installPlan}<div class="settings-card signin"><PiSignIn command={installPlan.loginCommand} onOpenTerminal={openTerminal} /></div>{/if}
          {:else if settingsSection === 'mcp'}
            <h1>MCP servers</h1><p class="lead">Connect tools and data sources through the Model Context Protocol. πDesk's Pi reaches them with one <code>mcp</code> tool, and servers start only when a thread uses them.</p>
            <McpServers onRunInTerminal={openTerminal} />
          {:else}
            <h1>Extensions</h1><p class="lead">Pi packages add tools, skills, prompts and themes to every thread. Search the <button class="text-link" onclick={() => void openExternal('https://pi.dev/packages')}>pi.dev gallery</button> or install one by name.</p>
            <Extensions onUpdates={count => extensionUpdates = count} />
          {/if}
        </div>
        {/key}
      </div>
    </div>
  </div>
{/if}
{#if internStarted}<PiIntern open={internOpen} {projects} threadProjectId={visibleThread?.projectId ?? null} onClose={() => internOpen = false} onPending={count => internPending = count} onAttention={() => internOpen = true}/>{/if}
{#if terminalOpen}
  <div class="overlay terminal-overlay" role="presentation">
    <div class="terminal-sheet dialog" role="dialog" aria-modal="true" aria-label="Pi terminal">
      <header><span><Terminal size={15}/> πDesk terminal</span><button class="icon-button" aria-label="Close terminal" onclick={() => terminalOpen = false}><X size={17}/></button></header>
      <PiTerminal cwd={activeProject?.path} run={terminalRun} />
    </div>
  </div>
{/if}
</svelte:boundary>

<style>
  .app-shell { display:flex; width:100vw; height:100vh; min-width:780px; background:var(--bg); overflow:hidden; }

  /* ---------- Sidebar ---------- */
  .sidebar { background:var(--sidebar); width:var(--sidebar-width); min-width:205px; max-width:390px; display:flex; flex-direction:column; flex-shrink:0; overflow:hidden; user-select:none; }
  .sidebar-top { height:var(--header-height); min-height:var(--header-height); display:flex; align-items:center; justify-content:flex-end; padding:0 10px 0 84px; }
  .sidebar-tools { display:flex; gap:2px; }
  .icon-button, .mini-button { border:0; background:transparent; color:var(--muted); display:inline-flex; align-items:center; justify-content:center; border-radius:var(--radius-sm); width:28px; height:28px; flex-shrink:0; transition:background .12s, color .12s; }
  .mini-button { width:22px; height:22px; }
  .icon-button:hover:not(:disabled), .mini-button:hover, .icon-button.pressed { background:var(--surface-2); color:var(--text); }
  .project-list { flex:1; overflow:auto; padding:4px 8px 12px; }
  .section-label { display:flex; align-items:center; justify-content:space-between; height:28px; padding:0 4px 0 10px; color:var(--subtle); font-size:11px; font-weight:600; }
  .section-label .mini-button { opacity:0; }
  .project-list:hover .section-label .mini-button, .section-label .mini-button:focus-visible { opacity:1; }
  .project-section { margin-bottom:2px; }
  .project-row { position:relative; display:flex; align-items:center; border-radius:var(--radius-sm); }
  .project-row:hover { background:color-mix(in srgb, var(--surface-2) 70%, transparent); }
  .project-toggle { flex:1; min-width:0; display:flex; align-items:center; gap:7px; border:0; background:transparent; color:var(--text); text-align:left; padding:6px 6px 6px 6px; font-weight:600; font-size:13px; }
  .project-toggle :global(.chev) { color:var(--subtle); transition:transform .15s var(--ease); flex-shrink:0; }
  .project-toggle :global(.chev.open) { transform:rotate(90deg); }
  .project-glyph { width:18px; height:18px; display:inline-flex; align-items:center; justify-content:center; border-radius:5px; color:var(--muted); background:var(--surface-2); font-size:10px; font-weight:700; flex-shrink:0; }
  .project-row.active .project-glyph { color:var(--on-accent); background:var(--accent-strong); }
  .project-name { flex:1; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .row-more, .thread-more { display:flex; opacity:0; width:22px; height:22px; border:0; border-radius:5px; background:transparent; color:var(--muted); align-items:center; justify-content:center; margin-right:4px; flex-shrink:0; }
  .project-row:hover .row-more, .row-more[aria-expanded='true'], .row-more:focus-visible { opacity:1; }
  .row-more:hover, .thread-more:hover { color:var(--text); background:var(--surface-3); }
  .project-menu { position:absolute; top:calc(100% + 4px); right:0; z-index:20; min-width:220px; max-width:260px; padding:4px; background:var(--elevated); border-radius:var(--radius); box-shadow:var(--shadow); animation:ui-pop .12s var(--ease); }
  .menu-caption { padding:6px 8px 7px; color:var(--subtle); font-size:11px; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; border-bottom:1px solid var(--line); margin-bottom:4px; }
  .project-menu button { width:100%; display:flex; align-items:center; gap:8px; border:0; background:transparent; padding:6px 8px; border-radius:5px; font-size:12.5px; text-align:left; }
  .project-menu button:hover { background:var(--surface-2); }
  .project-menu button.danger { color:var(--bad); }
  .project-menu button.danger:hover { background:var(--bad-bg); }
  .thread-menu { position:fixed; z-index:50; min-width:200px; }
  .menu-divider { height:1px; background:var(--line); margin:4px 0; }

  .thread-list { margin:2px 0 8px 12px; padding-left:8px; border-left:1px solid var(--line); }
  .new-thread-row { display:flex; align-items:center; gap:4px; margin-bottom:1px; }
  .new-thread { flex:1; display:flex; align-items:center; gap:8px; border:0; background:transparent; padding:5px 8px; border-radius:var(--radius-sm); color:var(--muted); font-size:12.5px; text-align:left; }
  .new-thread:hover:not(:disabled) { color:var(--text); background:color-mix(in srgb, var(--surface-2) 70%, transparent); }
  .new-thread :global(svg) { color:var(--accent); }
  .thread-row { display:flex; align-items:center; border-radius:var(--radius-sm); min-height:44px; margin:1px 0; }
  .thread-row > .status-dot { margin:0 9px 0 8px; }
  .thread-row:hover { background:color-mix(in srgb, var(--surface-2) 70%, transparent); }
  .thread-row.active { background:var(--surface-2); }
  .thread-row.active .thread-title { color:var(--text); font-weight:500; }
  .thread-row:hover .thread-more, .thread-row.active .thread-more, .thread-more:focus-visible { opacity:1; }
  .thread-link { flex:1; min-width:0; display:flex; align-items:center; gap:9px; height:100%; border:0; background:none; color:var(--muted); text-align:left; padding:0 6px 0 8px; font-size:12.5px; }
  .thread-title { flex:1; min-width:0; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .thread-text { flex:1; min-width:0; display:flex; flex-direction:column; gap:2px; padding:5px 0; }
  .thread-line { display:flex; align-items:center; gap:6px; min-width:0; }
  .thread-meta { display:flex; align-items:center; gap:6px; min-width:0; overflow:hidden; white-space:nowrap; font-size:11px; color:var(--subtle); font-variant-numeric:tabular-nums; }
  .meta-working { color:var(--accent); font-weight:500; }
  .meta-bad { color:var(--bad); font-weight:500; }
  .meta-done { color:var(--good); font-weight:500; }
  .meta-chip { flex:none; height:16px; padding:0 5px; border-radius:4px; background:var(--surface-2); color:var(--muted); font-size:10.5px; font-weight:500; line-height:16px; max-width:110px; overflow:hidden; text-overflow:ellipsis; }
  .meta-chip.mono { font-family:var(--mono); font-size:10px; }
  .meta-diff { flex:none; } .meta-diff .add { color:var(--good); } .meta-diff .del { color:var(--bad); }
  .row-pill { flex:none; position:relative; height:17px; padding:0 6px; border-radius:999px; background:var(--warn-bg); color:var(--warn); font-size:10px; font-weight:700; line-height:17px; animation:pill-pop .45s cubic-bezier(.34,1.36,.64,1) both; }
  .row-pill.plan::after { content:""; position:absolute; inset:-1px; border-radius:inherit; border:1.5px solid var(--warn); animation:pill-ring 1.4s ease-out .2s 2 both; }
  @keyframes pill-pop { from { transform:scale(.6); opacity:0; } }
  @keyframes pill-ring { from { opacity:.9; transform:scale(1); } to { opacity:0; transform:scale(1.4, 1.8); } }
  .pill-clock { margin-left:6px; opacity:.8; font-variant-numeric:tabular-nums; }
  .thread-link :global(.pin-mark) { color:var(--subtle); flex-shrink:0; }
  .threads-empty { padding:4px 8px 6px; color:var(--subtle); font-size:11.5px; }
  .thread-rename { flex:1; min-width:0; height:23px; padding:2px 5px; border:1px solid var(--line-strong); border-radius:5px; font-size:12px; background:var(--bg); color:var(--text); }
  .thread-rename:focus { border-color:var(--accent); box-shadow:var(--focus-ring); }

  .status-dot { position:relative; width:8px; height:8px; flex-shrink:0; border-radius:50%; background:transparent; box-shadow:inset 0 0 0 1.5px var(--subtle); }
  .status-dot.active { background:var(--accent); box-shadow:0 0 0 3px var(--accent-bg); animation:ui-pulse 1.8s ease-in-out infinite; }
  .status-dot.waiting { background:var(--warn); box-shadow:0 0 0 3px var(--warn-bg); }
  .status-dot.completed { background:var(--good); box-shadow:none; opacity:.85; }
  .status-dot.failed, .status-dot.disconnected { background:var(--bad); box-shadow:none; }
  .status-dot.idle { opacity:.7; }

  .empty-projects { padding:8px 10px; color:var(--subtle); font-size:12px; line-height:1.55; }
  .sidebar-bottom { display:flex; align-items:center; gap:2px; padding:8px; border-top:1px solid var(--line); }
  .footer-button { display:flex; align-items:center; gap:8px; padding:6px 8px; border:0; border-radius:var(--radius-sm); color:var(--muted); background:transparent; font-size:12.5px; }
  .footer-button:hover:not(:disabled) { color:var(--text); background:var(--surface-2); }
  .grow { flex:1; }

  .resize-handle { position:relative; width:1px; flex-shrink:0; cursor:col-resize; background:var(--line); }
  .resize-handle::after { content:''; position:absolute; inset:0 -3px; }
  .resize-handle:hover, .resize-handle:focus-visible { background:var(--accent); box-shadow:none; }

  /* ---------- Main ---------- */
  .main-pane { flex:1; min-width:0; display:flex; flex-direction:column; overflow:hidden; position:relative; background:var(--bg); container:main / inline-size; }
  .main-header { height:var(--header-height); min-height:var(--header-height); display:flex; align-items:center; gap:12px; padding:0 14px 0 20px; border-bottom:1px solid var(--line); user-select:none; }
  .crumbs { flex:1; min-width:0; display:flex; align-items:center; gap:6px; font-size:13px; white-space:nowrap; }
  .crumb-project { color:var(--muted); flex-shrink:0; }
  .crumbs :global(.crumb-sep) { color:var(--subtle); flex-shrink:0; }
  .top-thread { font-weight:600; overflow:hidden; text-overflow:ellipsis; min-width:0; }
  .status-pill { display:inline-flex; align-items:center; gap:6px; margin-left:6px; padding:2px 8px 2px 7px; border-radius:999px; background:var(--surface-2); color:var(--muted); font-size:11px; font-weight:500; flex-shrink:0; }
  .status-pill .status-dot { width:6px; height:6px; box-shadow:none; }
  .status-pill.active { color:var(--accent); background:var(--accent-bg); }
  .status-pill.waiting { color:var(--warn); background:var(--warn-bg); }
  .status-pill.completed { color:var(--good); background:var(--good-bg); }
  .status-pill.failed, .status-pill.disconnected { color:var(--bad); background:var(--bad-bg); }
  .header-actions { display:flex; align-items:center; gap:8px; flex-shrink:0; }
  .worktree-chip { display:inline-flex; align-items:center; gap:5px; min-width:0; max-width:240px; height:22px; margin-left:4px; padding:0 8px; border:1px solid var(--line-strong); border-radius:999px; background:var(--surface); color:var(--muted); font:500 11.5px var(--mono); -webkit-app-region:no-drag; }
  .worktree-chip span { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .worktree-chip :global(svg) { flex:none; color:var(--accent); }
  .worktree-chip:hover { background:var(--surface-2); color:var(--text); }
  .badge { display:inline-flex; align-items:center; gap:4px; height:20px; padding:0 7px; border-radius:5px; border:1px solid var(--line-strong); color:var(--muted); font-size:10.5px; font-weight:600; letter-spacing:.03em; }
  .panel-toggles { display:flex; gap:2px; padding:2px; border-radius:8px; background:var(--surface); border:1px solid var(--line); }
  .toggle-button { display:inline-flex; align-items:center; gap:6px; height:26px; padding:0 10px; border:0; border-radius:6px; background:transparent; color:var(--muted); font-size:12px; font-weight:500; }
  .toggle-button:hover { color:var(--text); }
  .toggle-button.pressed { background:var(--elevated); color:var(--text); box-shadow:var(--shadow-sm), 0 0 0 1px var(--line); }

  .status-bar { flex:none; display:flex; justify-content:flex-end; align-items:center; height:36px; padding:0 12px; }
  .details-pane { width:var(--panel-width); min-width:320px; max-width:850px; display:flex; flex-direction:column; overflow:hidden; background:var(--panel); animation:panel-arrive .16s var(--ease); }
  @keyframes panel-arrive { from { opacity:.4; transform:translateX(8px); } to { opacity:1; transform:none; } }
  .panel-loading { display:flex; align-items:center; justify-content:center; flex:1; gap:9px; color:var(--muted); font-size:12px; }

  .main-empty { flex:1; display:flex; flex-direction:column; align-items:center; justify-content:center; text-align:center; padding:30px; animation:ui-rise .25s var(--ease); }
  .main-empty.delayed { animation:ui-rise .2s var(--ease) .15s both; }
  .main-empty h2 { font-size:19px; font-weight:600; letter-spacing:-.02em; margin:16px 0 6px; }
  .main-empty p { color:var(--muted); margin:0 0 22px; font-size:13px; max-width:380px; line-height:1.55; }
  .main-empty strong { color:var(--text); font-weight:600; }
  .main-empty :global(.spin) { color:var(--accent); }
  .empty-graphic { width:56px; height:56px; display:flex; align-items:center; justify-content:center; color:var(--accent); background:var(--accent-bg); border-radius:16px; }
  .empty-graphic.danger { color:var(--bad); background:var(--bad-bg); }
  .empty-hint { display:flex; align-items:center; gap:4px; color:var(--subtle); font-size:11.5px; margin-top:22px; }
  .dot-sep { width:3px; height:3px; border-radius:50%; background:var(--subtle); margin:0 6px; }
  .primary-button, .secondary-button { display:inline-flex; align-items:center; justify-content:center; gap:7px; height:32px; padding:0 14px; border-radius:var(--radius); font-weight:600; font-size:13px; transition:filter .12s, background .12s; }
  .primary-button { background:var(--accent-strong); border:0; color:var(--on-accent); box-shadow:var(--shadow-sm); }
  .primary-button:hover:not(:disabled) { filter:brightness(1.08); }
  .secondary-button { background:var(--surface); border:1px solid var(--line-strong); color:var(--text); }
  .secondary-button:hover:not(:disabled) { background:var(--surface-2); }
  .secondary-button.small { height:26px; padding:0 10px; font-size:12px; font-weight:500; border-radius:var(--radius-sm); }

  .error-banner { display:flex; align-items:center; gap:10px; margin:10px 16px 0; padding:8px 8px 8px 12px; color:var(--bad); background:var(--bad-bg); border-radius:var(--radius); font-size:12.5px; animation:ui-rise .2s var(--ease); }
  .error-banner span { flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .error-banner button { display:flex; align-items:center; gap:5px; background:transparent; border:0; color:inherit; white-space:nowrap; padding:4px 8px; border-radius:var(--radius-sm); font-size:12px; font-weight:500; }
  .error-banner button:hover { background:color-mix(in srgb, var(--bad) 14%, transparent); }

  .text-button { display:inline-flex; align-items:center; gap:5px; border:0; background:none; color:var(--muted); font-size:12px; padding:3px 0; }
  .text-button:hover:not(:disabled) { color:var(--accent); }

  /* ---------- Dialogs ---------- */
  .overlay { position:fixed; inset:0; background:rgb(0 0 0 / .35); z-index:50; display:flex; align-items:flex-start; justify-content:center; padding-top:14vh; backdrop-filter:blur(2px); animation:fade-in .12s ease-out; }
  @keyframes fade-in { from { opacity:0; } to { opacity:1; } }
  .dialog { width:min(580px, calc(100vw - 48px)); background:var(--elevated); border-radius:var(--radius-lg); box-shadow:var(--shadow); overflow:hidden; animation:ui-pop .16s var(--ease); }
  .switch-search { height:54px; display:flex; align-items:center; gap:11px; padding:0 16px; border-bottom:1px solid var(--line); color:var(--subtle); }
  .switch-search input { background:transparent; border:0; outline:none; font-size:15px; flex:1; min-width:0; color:var(--text); }
  .switch-search input::placeholder { color:var(--subtle); }
  .switch-search input:focus-visible { box-shadow:none; }
  .switch-results { max-height:380px; overflow:auto; padding:6px; }
  .switch-results button { display:flex; align-items:center; gap:11px; width:100%; border:0; background:transparent; text-align:left; border-radius:var(--radius); padding:7px 10px; }
  .switch-results button.selected { background:var(--accent-bg); }
  .switch-text { flex:1; min-width:0; }
  .switch-results strong { display:block; font-size:13px; font-weight:500; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .switch-results small { display:block; color:var(--subtle); font-size:11.5px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .switch-icon { width:28px; height:28px; border-radius:7px; display:flex; align-items:center; justify-content:center; flex-shrink:0; color:var(--muted); background:var(--surface-2); }
  .switch-results button.selected .switch-icon { color:var(--accent); background:var(--elevated); }
  .switch-results p { padding:18px 14px; margin:0; color:var(--muted); text-align:center; }
  .dialog-footer { display:flex; gap:16px; padding:8px 14px; border-top:1px solid var(--line); font-size:11px; color:var(--subtle); }
  .dialog-footer span { display:flex; align-items:center; gap:4px; }

  .settings-page { position:fixed; inset:0; z-index:40; display:flex; background:var(--bg); animation:settings-in .22s var(--ease); }
  @keyframes settings-in { from { opacity:0; transform:scale(.985); } }
  .settings-nav { width:230px; flex:none; display:flex; flex-direction:column; gap:2px; padding:56px 12px 14px; background:var(--sidebar); border-right:1px solid var(--line); }
  .settings-nav-title { padding:0 10px 12px; font-size:18px; font-weight:650; letter-spacing:-.02em; }
  .settings-nav-item { display:flex; align-items:center; gap:10px; height:34px; padding:0 10px; border:0; border-radius:var(--radius); background:none; color:var(--muted); font-size:13px; font-weight:500; text-align:left; transition:background .12s, color .12s; }
  .settings-nav-item:hover { background:var(--surface-2); color:var(--text); }
  .settings-nav-item.on { background:var(--surface-2); color:var(--text); }
  .settings-nav-item.on :global(svg) { color:var(--accent); }
  .settings-nav-item span:first-of-type { flex:1; }
  .nav-badge { min-width:18px; height:18px; padding:0 5px; border-radius:999px; background:var(--warn-bg); color:var(--warn); font-size:11px; font-weight:700; line-height:18px; text-align:center; }
  .settings-nav-foot { padding:0 10px; color:var(--subtle); font-size:11px; }
  .settings-main { flex:1; min-width:0; display:flex; flex-direction:column; }
  .settings-top { height:var(--header-height); flex:none; display:flex; align-items:center; justify-content:flex-end; padding:0 14px; }
  .settings-scroll { flex:1; min-height:0; overflow:auto; }
  .settings-inner { max-width:760px; margin:0 auto; padding:8px 40px 56px; animation:ui-rise .2s var(--ease); }
  .settings-inner h1 { margin:0 0 6px; font-size:24px; font-weight:650; letter-spacing:-.025em; }
  .settings-inner h2 { margin:28px 0 8px; font-size:12px; font-weight:600; letter-spacing:.04em; text-transform:uppercase; color:var(--subtle); }
  .settings-inner .lead { margin:0 0 22px; color:var(--muted); font-size:13.5px; line-height:1.55; max-width:62ch; }
  .settings-inner p { color:var(--muted); font-size:12.5px; line-height:1.55; }
  .settings-card { padding:4px 16px; border:1px solid var(--line); border-radius:12px; background:var(--panel); }
  .settings-card.prose { padding:14px 16px; }
  .settings-card.prose p { margin:0; }
  .settings-card.signin { margin-top:16px; padding:16px; }
  .text-link { border:0; padding:0; background:none; color:var(--accent); font:inherit; }
  .text-link:hover { text-decoration:underline; }
  .settings-button { position:relative; }
  .gear-badge { position:absolute; top:5px; right:5px; width:7px; height:7px; border-radius:50%; background:var(--warn); box-shadow:0 0 0 2px var(--sidebar); }
  .setting-group { border:1px solid var(--line); border-radius:12px; padding:4px 16px; margin-bottom:12px; background:var(--panel); }
  .setting-row { display:flex; align-items:center; gap:12px; min-height:44px; }
  .setting-row > span:first-child { display:flex; align-items:center; gap:9px; margin-right:auto; white-space:nowrap; font-size:12.5px; }
  .setting-name :global(svg) { color:var(--subtle); }
  .segmented { display:flex; gap:2px; padding:2px; border-radius:8px; background:var(--surface); border:1px solid var(--line); }
  .segmented button { display:inline-flex; align-items:center; gap:5px; height:24px; padding:0 10px; border:0; border-radius:6px; background:transparent; color:var(--muted); font-size:12px; }
  .segmented button:hover { color:var(--text); }
  .segmented button.on { background:var(--elevated); color:var(--text); box-shadow:var(--shadow-sm), 0 0 0 1px var(--line); }
  .version { font-size:12px; font-weight:500; }
  .install-paths { display:grid; grid-template-columns:78px minmax(0,1fr); gap:7px 10px; padding:0 0 12px; font-size:11px; color:var(--muted); }
  .install-paths code { min-width:0; color:var(--text); font:11px/1.5 var(--mono); overflow-wrap:anywhere; user-select:text; }
  .install-actions { display:flex; gap:7px; flex-wrap:wrap; margin-bottom:10px; }
  .terminal-overlay { z-index:70; align-items:center; padding:4vh 4vw; }
  .terminal-sheet { width:min(1100px, 92vw); height:90vh; display:flex; flex-direction:column; }
  .terminal-sheet header { display:flex; align-items:center; justify-content:space-between; padding:10px 16px; border-bottom:1px solid var(--line); }
  .terminal-sheet header span { display:flex; align-items:center; gap:8px; font-size:13px; font-weight:600; }
  .missing { color:var(--subtle); font-size:12px; }
  .setting-select { height:26px; padding:0 8px; border:1px solid var(--line-strong); border-radius:var(--radius-sm); background:var(--elevated); color:var(--text); font-size:12px; }
  .error-details { white-space:pre-wrap; overflow:auto; max-height:52vh; padding:16px 20px; margin:0; font:11.5px/1.6 var(--mono); color:var(--muted); }

  /* Header adapts to the main pane's own width (side panels shrink it). */
  @container main (max-width: 760px) { .toggle-button span:not(.count) { display:none; } .toggle-button { padding:0 8px; } }
  @container main (max-width: 620px) { .crumb-project, .crumbs :global(.crumb-sep), .badge { display:none; } }
  @container main (max-width: 520px) { .status-pill { padding:0; width:18px; height:18px; justify-content:center; } .status-pill .pill-label { display:none; } }
</style>
