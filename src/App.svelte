<script lang="ts">
  import './app.css';
  import { onMount, tick } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { VList } from 'virtua/svelte';
  import { Plus, Settings2, PanelRightClose, GitCompareArrows, Network, Search, ChevronDown, ChevronRight, X, Pin, Archive, MoreHorizontal, RefreshCw, Terminal, FolderOpen, AlertTriangle, SunMoon, LoaderCircle, Check, SquarePen } from '@lucide/svelte';
  import { api, onBackendEvent } from '$lib/api';
  import { SessionModel } from '$lib/session.svelte';
  import Conversation from '$lib/components/conversation/Conversation.svelte';
  import type { BackendEvent, HarnessInstallation, HarnessKind, LoginProvider, Project, Thread, ThreadStatus, UiResponse } from '$lib/types';
  type AgentsPanelComponent = (typeof import('$lib/components/agents/AgentsPanel.svelte'))['default'];
  type ChangesPanelComponent = (typeof import('$lib/components/diff/ChangesPanel.svelte'))['default'];

  let projects = $state<Project[]>([]);
  let threadsByProject = $state<Record<string, Thread[]>>({});
  let harnesses = $state<HarnessInstallation[]>([]);
  let detecting = $state(true);
  let startupError = $state('');
  let crashDetails = $state<Record<string, string>>({});
  let errorDetailsOpen = $state(false);
  let activeProject = $state<Project | null>(null);
  let activeThread = $state<Thread | null>(null);
  let activeSession = $state<SessionModel | null>(null);
  let loadingThread = $state(false);
  let pendingAction = $state(false);
  let rightPanel = $state<'changes' | 'agents' | null>(null);
  let AgentsPanel = $state<AgentsPanelComponent | null>(null);
  let ChangesPanel = $state<ChangesPanelComponent | null>(null);
  let diffPath = $state<string | undefined>(undefined);
  let sidebarWidth = $state(256);
  let panelWidth = $state(405);
  let settingsOpen = $state(false);
  let switcherOpen = $state(false);
  let switchQuery = $state('');
  let switchIndex = $state(0);
  let showArchived = $state(false);
  let theme = $state<'system' | 'dark' | 'light'>('system');
  let installInProgress = $state<HarnessKind | null>(null);
  let installLog = $state<string[]>([]);
  let loginProviders = $state<LoginProvider[]>([]);
  let loggingIn = $state<string | null>(null);
  let newHarness = $state<HarnessKind>('omp');
  let renaming = $state<string | null>(null);
  let renameText = $state('');
  let sidebarResizing = false;
  let panelResizing = false;
  const liveSessions = new Map<string, SessionModel>();
  const idleStopTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const currentView = $derived(activeSession?.view);
  const availableKinds = $derived(new Set(harnesses.map(h => h.kind)));
  const onboarding = $derived(!detecting && harnesses.length === 0);
  const switchEntries = $derived(projects.flatMap(project => [
    { kind: 'project' as const, label: project.displayName, subtitle: project.path, project },
    ...(threadsByProject[project.id] ?? []).filter(t => !t.archived).map(thread => ({ kind: 'thread' as const, label: thread.title || 'New thread', subtitle: project.displayName, project, thread }))
  ]).filter(entry => `${entry.label} ${entry.subtitle}`.toLowerCase().includes(switchQuery.toLowerCase())).slice(0, 40));

  function errorText(error: unknown): string { return error instanceof Error ? error.message : String(error); }
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
    const timer = setTimeout(() => {
      idleStopTimers.delete(threadId);
      const view = liveSessions.get(threadId)?.view;
      if (activeThread?.id !== threadId && view &&
          view.status !== 'active' && view.status !== 'waiting' &&
          !view.agents.some(agent => agent.status === 'running' || agent.status === 'waiting')) {
        void api.stopThread(threadId).catch(() => {});
      }
    }, 30_000);
    idleStopTimers.set(threadId, timer);
  }
  function leaveCurrentThread(nextId?: string) {
    if (!activeThread || activeThread.id === nextId) return;
    const view = liveSessions.get(activeThread.id)?.view;
    if (view && view.status !== 'active' && view.status !== 'waiting') scheduleIdleStop(activeThread.id);
  }
  function cancelIdleStop(threadId: string) {
    const timer = idleStopTimers.get(threadId);
    if (timer) clearTimeout(timer);
    idleStopTimers.delete(threadId);
  }
  function handleBackendEvent(event: BackendEvent) {
    if (event.type === 'rpc') {
      const model = liveSessions.get(event.threadId);
      model?.apply(event.frame);
      const kind = event.frame.type;
      if (kind === 'agent_start') updateThreadStatus(event.threadId, 'active');
      if (kind === 'response' && event.frame.success === false && model?.view.status === 'failed') {
        updateThreadStatus(event.threadId, 'failed');
      }
      if (kind === 'agent_settled' || (kind === 'agent_end' && event.frame.isTerminal !== false && event.frame.willRetry !== true)) {
        const harness = kind === 'agent_end' ? (activeThread?.id === event.threadId ? activeThread.harness : Object.values(threadsByProject).flat().find(thread => thread.id === event.threadId)?.harness) : undefined;
        if (kind !== 'agent_end' || harness !== 'pi') {
          const runningChild = model?.view.agents.some(agent => agent.status === 'running');
          updateThreadStatus(event.threadId, model?.view.status === 'failed' ? 'failed' : runningChild ? 'active' : 'completed');
        }
      }
      if (kind === 'extension_ui_request' && ['select', 'confirm', 'input', 'editor'].includes(String(event.frame.method))) updateThreadStatus(event.threadId, 'waiting');
      if (kind === 'subagent_lifecycle' || kind === 'subagent_progress') {
        const runningChild = model?.view.agents.some(agent => agent.status === 'running');
        if (runningChild) updateThreadStatus(event.threadId, 'active');
      }
    } else if (event.type === 'exited') {
      if (!event.expected) {
        crashDetails[event.threadId] = event.stderr;
        liveSessions.get(event.threadId)?.setError(`${activeThread?.id === event.threadId ? activeThread.harness.toUpperCase() : 'Harness'} stopped unexpectedly. Your visible conversation is preserved.`);
        updateThreadStatus(event.threadId, 'disconnected');
      } else updateThreadStatus(event.threadId, 'idle');
    } else if (event.type === 'install_progress') {
      installLog = [...installLog.slice(-199), event.line];
    } else if (event.type === 'install_finished') {
      installInProgress = null;
      if (!event.success && event.error) installLog = [...installLog, event.error];
      void refreshHarnesses();
    }
  }
  onMount(() => {
    sidebarWidth = Number(localStorage.getItem('sidebarWidth')) || 256;
    panelWidth = Number(localStorage.getItem('panelWidth')) || 405;
    const savedTheme = localStorage.getItem('theme');
    if (savedTheme === 'light' || savedTheme === 'dark') theme = savedTheme;
    applyTheme();
    let unlisten: (() => void) | undefined;
    void onBackendEvent(handleBackendEvent).then(fn => { unlisten = fn; }).catch(err => { startupError = errorText(err); });
    void Promise.allSettled([refreshHarnesses(), refreshProjects()]).then(() => { detecting = false; });
    const onKey = (event: KeyboardEvent) => {
      if (!event.metaKey && event.key !== 'Escape') return;
      const key = event.key.toLowerCase();
      if (event.metaKey && key === 'k') { event.preventDefault(); void openSwitcher(); }
      else if (event.metaKey && key === 'n') { event.preventDefault(); if (activeProject) void createThread(activeProject); }
      else if (event.metaKey && event.shiftKey && key === 'd') { event.preventDefault(); togglePanel('changes'); }
      else if (event.metaKey && event.shiftKey && key === 'a') { event.preventDefault(); togglePanel('agents'); }
      else if (event.metaKey && key === ',') { event.preventDefault(); void openSettings(); }
      else if (event.key === 'Escape') {
        if (errorDetailsOpen) errorDetailsOpen = false;
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
    return () => { unlisten?.(); window.removeEventListener('keydown', onKey); window.removeEventListener('mousemove', move); window.removeEventListener('mouseup', up); for (const timer of idleStopTimers.values()) clearTimeout(timer); };
  });
  function applyTheme() { document.documentElement.dataset.theme = theme === 'system' ? '' : theme; localStorage.setItem('theme', theme); }
  async function refreshHarnesses() { try { harnesses = await api.detectHarnesses(); if (harnesses.length === 1) newHarness = harnesses[0].kind; } catch (error) { startupError = `Could not detect harnesses: ${errorText(error)}`; } }
  async function refreshProjects() {
    try {
      projects = await api.listProjects();
      const remembered = localStorage.getItem('lastProject');
      const selected = projects.find(p => p.id === remembered) ?? projects[0];
      if (selected) await selectProject(selected, true);
    } catch (error) { startupError = `Could not load projects: ${errorText(error)}`; }
  }
  async function refreshThreads(projectId: string) {
    try { threadsByProject[projectId] = await api.listThreads(projectId); }
    catch (error) { startupError = `Could not list threads: ${errorText(error)}`; }
  }
  async function selectProject(project: Project, restore = false) {
    leaveCurrentThread();
    activeProject = project;
    newHarness = project.preferredHarness;
    activeThread = null;
    activeSession = null;
    localStorage.setItem('lastProject', project.id);
    await refreshThreads(project.id);
    if (restore) {
      const savedId = localStorage.getItem('lastThread');
      const savedThread = threadsByProject[project.id]?.find(t => t.id === savedId);
      if (savedThread) await selectThread(savedThread);
    }
  }
  async function addProject() {
    const result = await open({ directory: true, multiple: false, title: 'Choose a project directory' });
    const path = Array.isArray(result) ? result[0] : result;
    if (!path) return;
    pendingAction = true;
    try {
      const project = await api.addProject(path, availableKinds.has(newHarness) ? newHarness : harnesses[0]?.kind ?? 'omp');
      const existing = projects.find(p => p.path === project.path);
      if (!existing) projects = [...projects, project];
      await selectProject(existing ?? project);
    } catch (error) { startupError = `Could not add project: ${errorText(error)}`; }
    finally { pendingAction = false; }
  }
  async function removeProject(project: Project) {
    if (!window.confirm(`Remove “${project.displayName}” from OMP Desktop?\n\nRepository files, Git history, and harness sessions will not be deleted.`)) return;
    try {
      await api.removeProject(project.id);
      projects = projects.filter(p => p.id !== project.id);
      if (activeProject?.id === project.id) { activeProject = null; activeThread = null; activeSession = null; if (projects[0]) await selectProject(projects[0]); }
    } catch (error) { startupError = errorText(error); }
  }
  async function createThread(project: Project) {
    pendingAction = true;
    try {
      const activePeer = (threadsByProject[project.id] ?? []).some(t => t.status === 'active' || t.status === 'waiting');
      const isolated = activePeer && project.isGit;
      const harness = availableKinds.has(newHarness) ? newHarness : harnesses[0]?.kind ?? project.preferredHarness;
      const thread = await api.createThread(project.id, harness, isolated);
      threadsByProject[project.id] = [thread, ...(threadsByProject[project.id] ?? []).filter(t => t.id !== thread.id)];
      if (activeProject?.id !== project.id) activeProject = project;
      await selectThread(thread);
    } catch (error) { startupError = `Could not start thread: ${errorText(error)}`; }
    finally { pendingAction = false; }
  }
  async function selectThread(thread: Thread) {
    leaveCurrentThread(thread.id);
    cancelIdleStop(thread.id);
    activeThread = thread;
    loadingThread = true;
    rightPanel = null;
    localStorage.setItem('lastThread', thread.id);
    try {
      const cached = liveSessions.get(thread.id);
      if (cached) activeSession = cached;
      else {
        const snapshot = await api.openThread(thread.id);
        activeThread = snapshot.thread;
        const model = new SessionModel(snapshot);
        liveSessions.set(thread.id, model);
        activeSession = model;
      }
      thread.lastViewedAt = new Date().toISOString();
    } catch (error) { startupError = `Could not open thread: ${errorText(error)}`; activeSession = null; }
    finally { loadingThread = false; }
  }
  async function openPanel(panel: 'changes' | 'agents') {
    if (panel === 'agents' && !activeSession?.view.capabilities.agents) return;
    rightPanel = panel;
    try {
      if (panel === 'agents' && !AgentsPanel) {
        AgentsPanel = (await import('$lib/components/agents/AgentsPanel.svelte')).default;
      } else if (panel === 'changes' && !ChangesPanel) {
        ChangesPanel = (await import('$lib/components/diff/ChangesPanel.svelte')).default;
      }
    } catch (error) {
      rightPanel = null;
      startupError = `Could not open ${panel}: ${errorText(error)}`;
    }
  }
  function togglePanel(panel: 'changes' | 'agents') {
    if (rightPanel === panel) rightPanel = null;
    else void openPanel(panel);
  }
  function showChanges(path?: string) {
    if (path && activeThread) {
      const cwd = activeThread.cwd.replace(/\/+$/, '');
      path = path.startsWith(`${cwd}/`) ? path.slice(cwd.length + 1) : path.replace(/^\.\//, '');
    }
    diffPath = path;
    void openPanel('changes');
  }
  async function send(message: string, mode: 'prompt' | 'steer' | 'follow_up') {
    if (!activeThread) return;
    try {
      await api.sendPrompt(activeThread.id, message, mode);
      startupError = '';
      if (!activeThread.title || activeThread.title === 'New thread') {
        const title = message.trim().split('\n')[0].slice(0, 70);
        if (title) await renameThread(activeThread, title);
      }
    } catch (error) { startupError = `Could not send message: ${errorText(error)}`; throw error; }
  }
  async function stop() { if (activeThread) { try { await api.abortThread(activeThread.id); } catch (error) { startupError = `Could not stop operation: ${errorText(error)}`; } } }
  async function respond(requestId: string, response: UiResponse) {
    if (!activeThread) return;
    try {
      await api.respondUi(activeThread.id, requestId, response);
      activeSession?.dismissRequest(requestId);
      updateThreadStatus(activeThread.id, 'active');
    }
    catch (error) { startupError = `Could not respond: ${errorText(error)}`; throw error; }
  }
  async function restart() {
    if (!activeThread) return;
    try {
      const snapshot = await api.restartThread(activeThread.id);
      const existing = liveSessions.get(activeThread.id);
      if (existing) {
        existing.reconnect(snapshot);
        activeSession = existing;
      } else {
        const restored = new SessionModel(snapshot);
        liveSessions.set(activeThread.id, restored);
        activeSession = restored;
      }
      Object.assign(activeThread, snapshot.thread);
      delete crashDetails[activeThread.id];
    } catch (error) { startupError = `Could not restart session: ${errorText(error)}`; }
  }
  async function renameThread(thread: Thread, title: string) {
    const clean = title.trim(); renaming = null;
    if (!clean) return;
    try {
      const updated = await api.renameThread(thread.id, clean);
      Object.assign(thread, updated);
      const rows = threadsByProject[updated.projectId];
      const row = rows?.find(candidate => candidate.id === updated.id);
      if (row) {
        Object.assign(row, updated);
        threadsByProject[updated.projectId] = [...rows];
      }
      if (activeThread?.id === updated.id) Object.assign(activeThread, updated);
    } catch (error) { startupError = errorText(error); }
  }
  async function setFlag(thread: Thread, kind: 'pinned' | 'archived') {
    try { Object.assign(thread, await api.setThreadFlags(thread.id, kind === 'pinned' ? !thread.pinned : undefined, kind === 'archived' ? !thread.archived : undefined)); }
    catch (error) { startupError = errorText(error); }
  }
  async function setModel(value: string) {
    if (!activeThread) return;
    const slash = value.indexOf('/'); if (slash < 0) return;
    try { const state = await api.setThreadModel(activeThread.id, value.slice(0, slash), value.slice(slash + 1)); if (activeSession && state.model) activeSession.view.model = state.model; if (activeSession) activeSession.view.effort = state.thinkingLevel; }
    catch (error) { startupError = errorText(error); }
  }
  async function setEffort(level: string) {
    if (!activeThread) return;
    try { const state = await api.setThreadEffort(activeThread.id, level); if (activeSession) activeSession.view.effort = state.thinkingLevel; }
    catch (error) { startupError = errorText(error); }
  }
  async function install(kind: HarnessKind) {
    installInProgress = kind; installLog = [];
    try { await api.installHarness(kind); await refreshHarnesses(); }
    catch (error) { installLog = [...installLog, errorText(error)]; }
    finally { installInProgress = null; }
  }
  async function locate(kind: HarnessKind) {
    const result = await open({ multiple: false, directory: false, title: `Locate ${kind.toUpperCase()} executable` });
    const path = Array.isArray(result) ? result[0] : result;
    if (!path) return;
    try { await api.setExecutableOverride(kind, path); await refreshHarnesses(); }
    catch (error) { startupError = `Invalid executable: ${errorText(error)}`; }
  }
  async function openSettings() {
    settingsOpen = true;
    loginProviders = [];
    if (activeThread?.harness === 'omp') {
      try { loginProviders = await api.getLoginProviders(activeThread.id); }
      catch (error) { startupError = `Could not load providers: ${errorText(error)}`; }
    }
  }
  async function login(providerId: string) {
    if (!activeThread) return;
    loggingIn = providerId;
    settingsOpen = false;
    try {
      await api.loginProvider(activeThread.id, providerId);
      loginProviders = await api.getLoginProviders(activeThread.id);
      settingsOpen = true;
    } catch (error) { startupError = `Sign-in failed: ${errorText(error)}`; }
    finally { loggingIn = null; }
  }
  async function openSwitcher() {
    switchQuery = ''; switchIndex = 0; switcherOpen = true;
    await Promise.all(projects.map(p => refreshThreads(p.id)));
    await tick();
    document.querySelector<HTMLInputElement>('#switcher-search')?.focus();
  }
  async function chooseSwitch(entry: (typeof switchEntries)[number]) {
    switcherOpen = false;
    await selectProject(entry.project);
    if (entry.kind === 'thread') await selectThread(entry.thread);
  }
  function statusMark(status: ThreadStatus): string {
    return ({ active: '●', waiting: '◉', idle: '◌', completed: '✓', failed: '!', disconnected: '!' } satisfies Record<ThreadStatus, string>)[status];
  }
  function focusInput(node: HTMLInputElement) { node.focus(); node.select(); }
</script>

<div class="app-shell" style={`--sidebar-width:${sidebarWidth}px; --panel-width:${panelWidth}px`}>
  <header class="topbar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region><div class="brand-mark" aria-hidden="true"><i></i><i></i><i></i></div><span>OMP<span class="brand-soft"> Desktop</span></span></div>
    <span class="bar-divider"></span>
    <div class="top-project" data-tauri-drag-region>{activeProject?.displayName ?? 'Workspace'}{#if activeThread}<ChevronRight size={13} strokeWidth={1.7} /><span class="top-thread">{activeThread.title}</span>{/if}</div>
    <div class="bar-spacer" data-tauri-drag-region></div>
    {#if activeThread && currentView}
      <span class="toolbar-harness">{activeThread.harness.toUpperCase()}</span>
      {#if activeThread.worktreePath}<span class="worktree-badge" title={`Isolated worktree: ${activeThread.worktreePath}`}>Isolated</span>{/if}
      {#if currentView.capabilities.modelSwitching}
        <label class="toolbar-select" aria-label="Model">
          <select value={currentView.model ? `${currentView.model.provider}/${currentView.model.id}` : ''} onchange={e => void setModel(e.currentTarget.value)} aria-label="Select model">
            {#if !currentView.model}<option value="">Default model</option>{/if}
            {#each currentView.models as model}<option value={`${model.provider}/${model.id}`}>{model.name} · {model.provider}</option>{/each}
          </select><ChevronDown size={12} strokeWidth={1.7} />
        </label>
      {/if}
      {#if currentView.capabilities.effortLevels && currentView.levels.length}
        <label class="toolbar-select effort" aria-label="Effort">
          <select value={currentView.effort ?? ''} onchange={e => void setEffort(e.currentTarget.value)} aria-label="Select effort">
            {#if !currentView.effort}<option value="">Default effort</option>{/if}
            {#each currentView.levels as level}<option value={level}>{level[0]?.toUpperCase() + level.slice(1)}</option>{/each}
          </select><ChevronDown size={12} strokeWidth={1.7} />
        </label>
      {/if}
      {#if currentView.contextUsage?.percent != null}<span class="context" title={`Context ${currentView.contextUsage.tokens ?? 0} / ${currentView.contextUsage.contextWindow} tokens`}>{Math.round(currentView.contextUsage.percent)}% context</span>{/if}
      <span class="bar-divider"></span>
      <button class:pressed={rightPanel === 'changes'} class="icon-button" title="Changes (⌘⇧D)" aria-label="Toggle changes" onclick={() => togglePanel('changes')}><GitCompareArrows size={17} strokeWidth={1.65} /></button>
      {#if currentView.capabilities.agents}<button class:pressed={rightPanel === 'agents'} class="icon-button" title="Agents (⌘⇧A)" aria-label="Toggle agents" onclick={() => togglePanel('agents')}><Network size={17} strokeWidth={1.65} /></button>{/if}
      {#if rightPanel}<button class="icon-button" title="Close panel" aria-label="Close side panel" onclick={() => rightPanel = null}><PanelRightClose size={16} strokeWidth={1.65} /></button>{/if}
    {/if}
    <button class="icon-button" title="Switch project or thread (⌘K)" aria-label="Switch project or thread" onclick={() => void openSwitcher()}><Search size={16} strokeWidth={1.7} /></button>
    <button class="icon-button" title="Settings (⌘,)" aria-label="Settings" onclick={() => void openSettings()}><Settings2 size={17} strokeWidth={1.65} /></button>
  </header>

  <div class="workspace">
    <aside class="sidebar" aria-label="Projects and threads">
      <div class="sidebar-heading"><span>PROJECTS</span><button class="mini-button" title="Add project" aria-label="Add project" onclick={() => void addProject()}><Plus size={16} strokeWidth={1.8} /></button></div>
      <div class="project-list">
        {#each projects as project (project.id)}
          <section class="project-section">
            <button class:active={activeProject?.id === project.id} class="project-row" onclick={() => void selectProject(project)} aria-label={`Open ${project.displayName}`}>
              <ChevronDown size={13} strokeWidth={1.8} class={activeProject?.id !== project.id ? 'rotated' : ''} /><span class="project-avatar">{project.displayName[0]?.toUpperCase()}</span><span class="project-name">{project.displayName}</span>
              {#if project.isGit}<span class="git-tick" title="Git repository">⌁</span>{/if}
            </button>
            {#if activeProject?.id === project.id}
              {@const visible = visibleThreads(project.id)}
              <div class="thread-list">
                <button class="new-thread" disabled={pendingAction || !harnesses.length} onclick={() => void createThread(project)}><Plus size={13} strokeWidth={2} /> New thread <kbd>⌘N</kbd></button>
                {#if harnesses.length > 1}
                  <label class="thread-harness-picker"><span>Harness</span><select aria-label="Harness for new threads" bind:value={newHarness}><option value="omp">OMP</option><option value="pi">Pi</option></select></label>
                {/if}
                {#if visible.length}
                  <VList data={visible} getKey={thread => thread.id} style={`height: min(55vh, ${visible.length * 31 + (renaming ? 90 : 0)}px);`}>
                    {#snippet children(thread)}
                      <div class:active={activeThread?.id === thread.id} class="thread-row">
                        <button class="thread-link" onclick={() => void selectThread(thread)} title={thread.title}>
                          <span class={`status-icon ${thread.status}`} aria-label={thread.status}>{statusMark(thread.status)}</span><span class="thread-title">{thread.title || 'New thread'}</span>
                        </button>
                        <button class="thread-more" title={`Actions for ${thread.title}`} aria-label={`Actions for ${thread.title}`} onclick={() => { renaming = renaming === thread.id ? null : thread.id; renameText = thread.title; }}><MoreHorizontal size={15} /></button>
                      </div>
                      {#if renaming === thread.id}
                        <div class="thread-actions">
                          <form onsubmit={event => { event.preventDefault(); void renameThread(thread, renameText); }}><input use:focusInput aria-label="Thread title" bind:value={renameText} maxlength="120" /><button title="Save title" aria-label="Save title"><Check size={14}/></button></form>
                          <button onclick={() => void setFlag(thread, 'pinned')}><Pin size={12}/>{thread.pinned ? 'Unpin' : 'Pin'}</button>
                          <button onclick={() => void setFlag(thread, 'archived')}><Archive size={12}/>{thread.archived ? 'Unarchive' : 'Archive'}</button>
                        </div>
                      {/if}
                    {/snippet}
                  </VList>
                {/if}
              </div>
            {/if}
          </section>
        {/each}
        {#if !projects.length && !detecting}<div class="empty-projects">Projects keep sessions connected to the code they change.</div>{/if}
      </div>
      <div class="sidebar-bottom">
        {#if projects.length}<button class="footer-link" onclick={() => showArchived = !showArchived}><Archive size={14} strokeWidth={1.8}/>{showArchived ? 'Hide archived' : 'Show archived'}</button>{/if}
        <button class="add-project" disabled={pendingAction} onclick={() => void addProject()}><Plus size={16} strokeWidth={1.8} /> Add project</button>
        {#if activeProject}<button class="footer-link remove-project" onclick={() => void removeProject(activeProject!)}>Remove from app</button>{/if}
      </div>
    </aside>
    <div class="resize-handle" role="slider" tabindex="0" aria-orientation="vertical" aria-valuemin="205" aria-valuemax="390" aria-valuenow={sidebarWidth} aria-label="Resize project sidebar" onmousedown={() => sidebarResizing = true} onkeydown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); sidebarWidth = Math.max(205, Math.min(390, sidebarWidth + (event.key === 'ArrowRight' ? 12 : -12))); localStorage.setItem('sidebarWidth', String(sidebarWidth)); } }}></div>

    <main class="main-pane">
      {#if startupError}<div class="error-banner" role="alert"><AlertTriangle size={15}/><span>{startupError}</span><button aria-label="Dismiss error" onclick={() => startupError = ''}><X size={14}/></button></div>{/if}
      {#if detecting}<div class="main-empty"><LoaderCircle class="spin" size={28} strokeWidth={1.4}/><h2>Finding your coding harnesses</h2><p>Checking OMP, Pi, and your shell environment.</p></div>
      {:else if onboarding}
        <div class="onboarding"><div class="onboarding-eyebrow">GET STARTED</div><h1>Your agents, in focus.</h1><p class="onboarding-intro">A quiet home for OMP and Pi sessions, tool activity, subagents, and code review. Your code and sessions stay on this Mac.</p>
          <div class="harness-choices">
            <div class="harness-card"><div class="harness-icon">O</div><div><h3>Oh My Pi</h3><p>Rich agent orchestration and tooling.</p></div><button disabled={installInProgress !== null} onclick={() => void install('omp')}>{installInProgress === 'omp' ? 'Installing…' : 'Install OMP'}</button></div>
            <div class="harness-card"><div class="harness-icon">π</div><div><h3>Pi</h3><p>The lightweight coding-agent foundation.</p></div><button disabled={installInProgress !== null} onclick={() => void install('pi')}>{installInProgress === 'pi' ? 'Installing…' : 'Install Pi'}</button></div>
          </div>
          <div class="install-notice">Installation runs only after you choose it. OMP: <code>bun install -g @oh-my-pi/pi-coding-agent</code><br />Pi: <code>npm install -g --ignore-scripts @earendil-works/pi-coding-agent</code></div>
          {#if installLog.length}<pre class="install-log" aria-live="polite">{installLog.join('\n')}</pre>{/if}
          <div class="onboarding-actions"><button onclick={() => void locate('omp')}>Locate OMP executable</button><button onclick={() => void locate('pi')}>Locate Pi executable</button><button onclick={() => void refreshHarnesses()}><RefreshCw size={14}/> Retry detection</button></div>
        </div>
      {:else if !activeProject}
        <div class="main-empty"><div class="empty-graphic"><FolderOpen size={32} strokeWidth={1.2}/></div><h2>Start with a project</h2><p>Choose a local directory. Nothing is uploaded or copied.</p><button class="primary-button" onclick={() => void addProject()}><Plus size={16}/> Add project</button></div>
      {:else if loadingThread}
        <div class="main-empty"><LoaderCircle class="spin" size={28} strokeWidth={1.4}/><h2>Opening thread</h2><p>Restoring the conversation from {activeThread?.harness.toUpperCase()}.</p></div>
      {:else if !activeThread || !currentView}
        <div class="main-empty"><div class="empty-graphic"><SquarePen size={30} strokeWidth={1.2}/></div><h2>What shall we work on?</h2><p>Start a thread in <strong>{activeProject.displayName}</strong> to talk to your agent.</p><button class="primary-button" disabled={pendingAction || !harnesses.length} onclick={() => void createThread(activeProject!)}><Plus size={16}/> New thread</button><div class="empty-hint">Your existing sessions are in the sidebar.</div></div>
      {:else}
        {#if currentView.error}<div class="error-banner"><AlertTriangle size={15}/><span>{currentView.error}</span>{#if crashDetails[activeThread.id]}<button onclick={() => errorDetailsOpen = true}>View details</button>{/if}<button onclick={() => void restart()}><RefreshCw size={13}/> Restart session</button></div>{/if}
        <Conversation view={currentView} onSend={send} onAbort={stop} onShowChanges={showChanges} onShowAgents={() => void openPanel('agents')} onRespond={respond} />
      {/if}
    </main>

    {#if rightPanel && activeThread && currentView}
      <div class="resize-handle panel-handle" role="slider" tabindex="0" aria-orientation="vertical" aria-valuemin="320" aria-valuemax="850" aria-valuenow={panelWidth} aria-label="Resize detail panel" onmousedown={() => panelResizing = true} onkeydown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); panelWidth = Math.max(320, Math.min(850, panelWidth + (event.key === 'ArrowLeft' ? 12 : -12))); localStorage.setItem('panelWidth', String(panelWidth)); } }}></div>
      <aside class="details-pane" aria-label={rightPanel === 'agents' ? 'Agents' : 'Changes'}>
        {#if rightPanel === 'agents'}
          {#if AgentsPanel}<AgentsPanel view={currentView} onClose={() => rightPanel = null} />
          {:else}<div class="panel-loading"><LoaderCircle class="spin" size={18}/><span>Loading agents…</span></div>{/if}
        {:else if ChangesPanel}<ChangesPanel thread={activeThread} onClose={() => rightPanel = null} focusPath={diffPath} />
        {:else}<div class="panel-loading"><LoaderCircle class="spin" size={18}/><span>Loading changes…</span></div>{/if}
      </aside>
    {/if}
  </div>
</div>

{#if errorDetailsOpen && activeThread}
  <div class="overlay" role="presentation" onclick={event => { if (event.target === event.currentTarget) errorDetailsOpen = false; }}>
    <div class="settings dialog" role="dialog" aria-modal="true" aria-label="Harness error details">
      <header><div><div class="dialog-eyebrow">HARNESS ERROR</div><h2>Details</h2></div><button class="icon-button" aria-label="Close details" onclick={() => errorDetailsOpen = false}><X size={18}/></button></header>
      <pre class="error-details">{crashDetails[activeThread.id]}</pre>
    </div>
  </div>
{/if}

{#if switcherOpen}
  <div class="overlay" role="presentation" onclick={event => { if (event.target === event.currentTarget) switcherOpen = false; }}>
    <div class="switcher dialog" role="dialog" aria-modal="true" aria-label="Switch project or thread">
      <div class="switch-search">
        <Search size={18}/>
        <input id="switcher-search" placeholder="Go to a project or thread…" bind:value={switchQuery}
          oninput={() => switchIndex = 0}
          onkeydown={event => {
            if (event.key === 'ArrowDown') { event.preventDefault(); switchIndex = Math.min(switchEntries.length - 1, switchIndex + 1); }
            if (event.key === 'ArrowUp') { event.preventDefault(); switchIndex = Math.max(0, switchIndex - 1); }
            if (event.key === 'Enter' && switchEntries[switchIndex]) { event.preventDefault(); void chooseSwitch(switchEntries[switchIndex]); }
          }} />
        <kbd>ESC</kbd>
      </div>
      <div class="switch-results">
        {#each switchEntries as entry, index}
          <button class:selected={switchIndex === index} onclick={() => void chooseSwitch(entry)}>
            <span class="switch-icon">{entry.kind === 'project' ? '◫' : '◌'}</span>
            <span><strong>{entry.label}</strong><small>{entry.subtitle}</small></span>
          </button>
        {/each}
        {#if !switchEntries.length}<p>No matching projects or threads.</p>{/if}
      </div>
      <div class="dialog-footer"><span><kbd>↑</kbd> <kbd>↓</kbd> navigate&nbsp;&nbsp; <kbd>↵</kbd> open</span><span>⌘K</span></div>
    </div>
  </div>
{/if}

{#if settingsOpen}
  <div class="overlay" role="presentation" onclick={event => { if (event.target === event.currentTarget) settingsOpen = false; }}>
    <div class="settings dialog" role="dialog" aria-modal="true" aria-label="Settings">
      <header><div><div class="dialog-eyebrow">PREFERENCES</div><h2>Settings</h2></div><button class="icon-button" aria-label="Close settings" onclick={() => settingsOpen = false}><X size={18}/></button></header>
      <section><h3>Appearance</h3><p>Choose a theme for this Mac.</p><label class="setting-row"><span><SunMoon size={17}/> Theme</span><select bind:value={theme} onchange={applyTheme}><option value="system">Follow system</option><option value="dark">Dark</option><option value="light">Light</option></select></label></section>
      <section>
        <h3>Coding harnesses</h3><p>System installations are used directly, including their existing sessions and credentials.</p>
        {#each ['omp', 'pi'] as kind}
          {@const installation = harnesses.find(h => h.kind === kind)}
          <div class="setting-row"><span><Terminal size={17}/> {kind.toUpperCase()}</span>
            {#if installation}<span class="install-path" title={installation.path}>{installation.version}<small>{installation.path}</small></span>
            {:else}<span class="missing">Not found</span>{/if}
            <button onclick={() => void locate(kind as HarnessKind)}>Choose…</button>
          </div>
        {/each}
        <button class="text-button" onclick={() => void refreshHarnesses()}><RefreshCw size={14}/> Scan again</button>
      </section>
      {#if activeThread?.harness === 'omp'}
        <section>
          <h3>Provider sign-in</h3><p>OMP manages credentials. This app never stores provider tokens.</p>
          {#each loginProviders.filter(provider => provider.available) as provider}
            <div class="setting-row">
              <span>{provider.name}</span>
              <span class:authenticated={provider.authenticated} class="provider-state">{provider.authenticated ? 'Connected' : 'Not connected'}</span>
              {#if !provider.authenticated}<button disabled={loggingIn !== null} onclick={() => void login(provider.id)}>{loggingIn === provider.id ? 'Signing in…' : 'Sign in'}</button>{/if}
            </div>
          {/each}
          {#if loginProviders.length === 0}<p>Open an OMP thread to view sign-in providers.</p>{/if}
        </section>
      {/if}
      <section><h3>Privacy</h3><p>Project metadata is stored locally. OMP and Pi retain control of sessions, credentials, extensions, and provider connections. This app sends no product telemetry.</p></section>
    </div>
  </div>
{/if}

<style>
  .app-shell { display:flex; flex-direction:column; width:100vw; height:100vh; min-width:780px; background:var(--bg); overflow:hidden; }
  .topbar { height:54px; min-height:54px; display:flex; align-items:center; gap:10px; border-bottom:1px solid var(--line); padding:0 18px 0 78px; background:var(--bg); user-select:none; }
  .brand { display:flex; align-items:center; gap:9px; font-size:13px; font-weight:700; letter-spacing:-.035em; white-space:nowrap; }
  .brand-soft { font-weight:500; color:var(--muted); }
  .brand-mark { width:18px; height:18px; display:flex; gap:2px; align-items:flex-end; transform:skew(-12deg); }
  .brand-mark i { display:block; width:4px; height:11px; background:var(--accent); border-radius:2px 2px 0 0; }
  .brand-mark i:nth-child(2) { height:16px; opacity:.8; }
  .brand-mark i:nth-child(3) { height:8px; opacity:.55; }
  .bar-divider { height:17px; width:1px; background:var(--line); margin:0 6px; flex-shrink:0; }
  .top-project { display:flex; align-items:center; gap:8px; font-weight:600; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; max-width:25vw; }
  .top-project :global(svg) { color:var(--subtle); flex-shrink:0; }
  .top-thread { color:var(--muted); font-weight:400; overflow:hidden; text-overflow:ellipsis; }
  .bar-spacer { flex:1; }
  .toolbar-harness { font-size:10px; letter-spacing:.08em; font-weight:700; color:var(--accent); background:var(--accent-bg); padding:3px 6px; border-radius:4px; }
  .worktree-badge { color:var(--subtle); font-size:10px; border:1px solid var(--line); border-radius:4px; padding:2px 5px; }
  .toolbar-select { display:flex; align-items:center; gap:3px; position:relative; max-width:195px; color:var(--muted); }
  .toolbar-select select { appearance:none; border:0; background:transparent; color:inherit; padding:4px 3px; max-width:170px; text-overflow:ellipsis; font-size:12px; cursor:pointer; }
  .toolbar-select.effort { max-width:110px; }.toolbar-select.effort select { max-width:85px; }
  .toolbar-select :global(svg) { flex-shrink:0; pointer-events:none; }.toolbar-select:hover { color:var(--text); }
  .context { color:var(--subtle); font-size:11px; white-space:nowrap; }
  .icon-button, .mini-button { border:0; background:transparent; color:var(--muted); display:inline-flex; align-items:center; justify-content:center; border-radius:5px; width:29px; height:29px; flex-shrink:0; }
  .icon-button:hover,.mini-button:hover,.icon-button.pressed { background:var(--surface-2); color:var(--text); }
  .workspace { flex:1; min-height:0; display:flex; }
  .sidebar { background:var(--sidebar); width:var(--sidebar-width); min-width:205px; max-width:390px; display:flex; flex-direction:column; flex-shrink:0; overflow:hidden; }
  .sidebar-heading { display:flex; align-items:center; justify-content:space-between; height:55px; padding:0 16px 0 19px; color:var(--subtle); font-size:10px; font-weight:700; letter-spacing:.14em; }
  .project-list { flex:1; overflow:auto; padding:0 9px; }
  .project-section { margin-bottom:11px; }
  .project-row { width:100%; display:flex; align-items:center; gap:8px; border:0; background:transparent; color:var(--text); text-align:left; border-radius:6px; padding:7px 7px; font-weight:650; }
  .project-row:hover,.project-row.active { background:var(--surface); }
  .project-row :global(svg) { color:var(--subtle); transition:transform .15s; flex-shrink:0; }
  .project-row :global(svg.rotated) { transform:rotate(-90deg); }
  .project-avatar { width:20px; height:20px; display:inline-flex; align-items:center; justify-content:center; border-radius:5px; color:var(--accent); background:var(--accent-bg); font-size:11px; flex-shrink:0; }
  .project-name { flex:1; overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }.git-tick { color:var(--subtle); font-size:19px; }
  .thread-list { margin:3px 0 0 21px; }.new-thread { width:100%; display:flex; align-items:center; gap:9px; border:0; background:transparent; padding:6px 8px; border-radius:5px; color:var(--muted); font-size:12px; text-align:left; }.new-thread:hover { color:var(--text); background:var(--surface); }.new-thread :global(svg) { color:var(--accent); }
  .new-thread kbd { margin-left:auto; color:var(--subtle); font-family:inherit; font-size:11px; }
  .thread-harness-picker { display:flex; align-items:center; justify-content:space-between; padding:2px 8px 5px 29px; color:var(--subtle); font-size:10px; }
  .thread-harness-picker select { color:var(--muted); border:0; background:transparent; font-size:10px; cursor:pointer; }
  .thread-row { display:flex; align-items:center; border-radius:5px; margin:1px 0; height:29px; }.thread-row:hover,.thread-row.active { background:var(--surface); }.thread-row.active .thread-title { color:var(--text); }
  .thread-link { flex:1; min-width:0; display:flex; align-items:center; gap:8px; height:100%; border:0; background:none; color:var(--muted); text-align:left; padding:0 8px; font-size:12px; }.thread-title { overflow:hidden; white-space:nowrap; text-overflow:ellipsis; }
  .thread-more { display:none; width:23px; height:23px; border:0; border-radius:4px; background:transparent; color:var(--muted); align-items:center; justify-content:center; margin-right:3px; }.thread-row:hover .thread-more,.thread-row.active .thread-more { display:flex; }.thread-more:hover { color:var(--text); background:var(--surface-2); }
  .status-icon { font-size:12px; width:13px; flex-shrink:0; color:var(--subtle); text-align:center; }.status-icon.active { color:var(--good); }.status-icon.waiting { color:var(--warn); }.status-icon.completed { color:var(--good); }.status-icon.failed,.status-icon.disconnected { color:var(--bad); font-weight:800; }
  .status-icon.active { animation: working-pulse 2.4s ease-in-out infinite; }
  @keyframes working-pulse { 50% { opacity: .55; } }
  .thread-actions { margin:3px 0 7px 18px; background:var(--surface); border:1px solid var(--line); border-radius:7px; padding:7px; display:grid; grid-template-columns:1fr 1fr; gap:4px; }.thread-actions form { grid-column:span 2; display:flex; }.thread-actions input { background:var(--bg); border:1px solid var(--line); padding:4px 6px; min-width:0; flex:1; border-radius:4px; }.thread-actions button { display:flex; align-items:center; justify-content:center; gap:5px; border:0; border-radius:4px; background:var(--surface-2); color:var(--muted); padding:4px; font-size:11px; }.thread-actions button:hover { color:var(--text); }
  .sidebar-bottom { padding:10px; border-top:1px solid var(--line); }.footer-link { width:100%; display:flex; align-items:center; gap:9px; padding:7px 8px; border:0; color:var(--muted); background:transparent; font-size:12px; text-align:left; border-radius:5px; }.footer-link:hover { color:var(--text); background:var(--surface); }
  .add-project { width:100%; display:flex; align-items:center; gap:9px; padding:9px 10px; margin-top:4px; border:1px solid var(--line); border-radius:6px; color:var(--muted); background:var(--surface); text-align:left; font-size:12px; }.add-project:hover { border-color:var(--accent); color:var(--text); }.remove-project { color:var(--subtle); font-size:11px; }
  .empty-projects { padding:8px 10px; color:var(--subtle); font-size:12px; line-height:1.6; }
  .resize-handle { width:4px; flex-shrink:0; cursor:col-resize; background:var(--line); opacity:.45; }.resize-handle:hover { background:var(--accent); opacity:1; }.panel-handle { background:var(--line); }
  .main-pane { flex:1; min-width:0; display:flex; flex-direction:column; overflow:hidden; position:relative; }.details-pane { width:var(--panel-width); min-width:320px; max-width:850px; display:flex; flex-direction:column; overflow:hidden; background:var(--sidebar); }
  .details-pane { animation: panel-arrive .14s ease-out; }
  @keyframes panel-arrive { from { opacity: .65; } to { opacity: 1; } }
  .panel-loading { display:flex; align-items:center; justify-content:center; flex:1; gap:9px; color:var(--muted); font-size:12px; }
  .main-empty { display:flex; flex-direction:column; align-items:center; justify-content:center; height:100%; text-align:center; padding:30px; }.main-empty h2 { font-size:20px; font-weight:600; letter-spacing:-.035em; margin:15px 0 5px; }.main-empty p { color:var(--muted); margin:0 0 20px; font-size:13px; }.main-empty strong { color:var(--text); }.empty-graphic { width:63px; height:63px; display:flex; align-items:center; justify-content:center; color:var(--accent); background:var(--accent-bg); border-radius:15px; }.empty-hint { color:var(--subtle); font-size:11px; margin-top:19px; }
  .primary-button { display:inline-flex; align-items:center; gap:7px; background:var(--accent); border:1px solid var(--accent); color:var(--bg); border-radius:6px; font-weight:700; padding:8px 13px; }.primary-button:hover { filter:brightness(1.08); }
  .error-banner { display:flex; align-items:center; gap:10px; padding:9px 15px; color:var(--bad); background:color-mix(in srgb,var(--bad) 10%,var(--bg)); border-bottom:1px solid color-mix(in srgb,var(--bad) 22%,var(--line)); font-size:12px; }.error-banner span { flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }.error-banner button { display:flex; align-items:center; gap:5px; background:transparent; border:0; color:inherit; white-space:nowrap; }
  .spin { animation:spin 1.5s linear infinite; color:var(--accent); }@keyframes spin { to { transform:rotate(360deg); } }
  .onboarding { width:min(640px,calc(100% - 60px)); align-self:center; margin:auto; max-height:100%; overflow:auto; padding:30px 0; }.onboarding-eyebrow,.dialog-eyebrow { color:var(--accent); font-size:10px; font-weight:700; letter-spacing:.16em; }.onboarding h1 { font-size:32px; margin:8px 0 9px; font-weight:650; letter-spacing:-.05em; }.onboarding-intro { color:var(--muted); line-height:1.7; max-width:480px; margin:0 0 34px; font-size:13px; }.harness-choices { display:grid; gap:9px; }.harness-card { display:flex; align-items:center; gap:14px; padding:14px; background:var(--surface); border:1px solid var(--line); border-radius:8px; }.harness-icon { width:38px; height:38px; flex-shrink:0; display:flex; align-items:center; justify-content:center; border-radius:8px; background:var(--accent-bg); color:var(--accent); font-size:21px; font-weight:700; }.harness-card div:nth-child(2) { flex:1; }.harness-card h3 { margin:0 0 2px; font-size:14px; }.harness-card p { margin:0; color:var(--muted); font-size:11px; }.harness-card button { border:1px solid var(--line); border-radius:5px; background:var(--surface-2); padding:7px 10px; font-size:12px; }.harness-card button:hover { border-color:var(--accent); }.install-notice { color:var(--subtle); font-size:11px; line-height:1.8; margin:19px 0; }.install-notice code { color:var(--muted); }.install-log { background:#090b0e; color:var(--muted); border:1px solid var(--line); border-radius:6px; padding:11px; max-height:150px; overflow:auto; font-size:10px; white-space:pre-wrap; }.onboarding-actions { display:flex; flex-wrap:wrap; gap:14px; margin-top:21px; }.onboarding-actions button,.text-button { display:inline-flex; align-items:center; gap:5px; border:0; background:none; color:var(--muted); font-size:11px; padding:3px 0; }.onboarding-actions button:hover,.text-button:hover { color:var(--accent); }
  .overlay { position:fixed; inset:0; background:#0009; z-index:30; display:flex; align-items:flex-start; justify-content:center; padding-top:16vh; backdrop-filter:blur(3px); }.dialog { width:min(560px,calc(100vw - 50px)); background:var(--surface); border:1px solid var(--line); border-radius:10px; box-shadow:var(--shadow); }.switch-search { height:52px; display:flex; align-items:center; gap:10px; padding:0 14px; border-bottom:1px solid var(--line); color:var(--subtle); }.switch-search input { background:transparent; border:0; outline:none; font-size:14px; flex:1; min-width:0; }.switch-search kbd,.dialog-footer kbd { font-family:inherit; border:1px solid var(--line); border-radius:3px; padding:1px 4px; color:var(--subtle); font-size:10px; }.switch-results { max-height:360px; overflow:auto; padding:6px; }.switch-results button { display:flex; align-items:center; gap:12px; width:100%; border:0; background:transparent; text-align:left; border-radius:5px; padding:8px 10px; }.switch-results button:hover,.switch-results button:focus-visible { background:var(--surface-2); }.switch-results strong { display:block; font-size:12px; }.switch-results small { display:block; color:var(--subtle); font-size:11px; }.switch-icon { color:var(--accent); font-size:18px; }.switch-results p { padding:14px; color:var(--muted); }.dialog-footer { display:flex; justify-content:space-between; padding:8px 13px; border-top:1px solid var(--line); font-size:10px; color:var(--subtle); }
  .switch-results button.selected { background:var(--surface-2); }
  .settings { max-height:74vh; overflow:auto; }
  .settings header { display:flex; align-items:center; justify-content:space-between; padding:18px 20px; border-bottom:1px solid var(--line); }
  .settings h2 { font-size:19px; margin:4px 0 0; letter-spacing:-.03em; }
  .settings section { padding:15px 20px 18px; border-bottom:1px solid var(--line); }
  .settings section:last-child { border:0; }
  .settings h3 { font-size:12px; margin:0 0 3px; }
  .settings p { color:var(--muted); font-size:11px; line-height:1.6; margin:0 0 15px; }
  .setting-row { display:flex; align-items:center; gap:12px; padding:7px 0; min-height:41px; }
  .setting-row > span:first-child { display:flex; align-items:center; gap:9px; margin-right:auto; white-space:nowrap; }
  .setting-row > span:first-child :global(svg) { color:var(--subtle); }
  .setting-row select,.setting-row button { background:var(--surface-2); border:1px solid var(--line); border-radius:5px; padding:5px 8px; font-size:11px; }
  .setting-row button:hover { border-color:var(--accent); }
  .install-path { font-size:11px; text-align:right; }
  .install-path small { display:block; max-width:190px; color:var(--subtle); white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
  .missing { color:var(--subtle); font-size:11px; }
  .error-details { white-space:pre-wrap; overflow:auto; max-height:50vh; padding:15px 20px; margin:0; font:11px/1.55 var(--mono); color:var(--muted); }
  .provider-state { font-size:11px; color:var(--subtle); }.provider-state.authenticated { color:var(--good); }
  @media (max-width: 1050px) { .top-thread,.context,.toolbar-select.effort { display:none; } }
  @media (max-width: 850px) { .brand-soft,.toolbar-harness { display:none; } }
</style>
