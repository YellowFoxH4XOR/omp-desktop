<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { Bot, X, Eraser, ClipboardList, Paperclip, ArrowUp, Square, ShieldCheck, RefreshCw, FolderPlus, Folder, Globe, Check } from '@lucide/svelte';
  import { api, onBackendEvent } from '$lib/api';
  import { SessionModel } from '$lib/session.svelte';
  import type { BackendEvent, InternPlan, Project, UiResponse } from '$lib/types';
  import Transcript from '$lib/components/conversation/Transcript.svelte';
  import RequestCard from '$lib/components/conversation/RequestCard.svelte';
  import PlanReview from '$lib/components/plan/PlanReview.svelte';
  import { planOf, PLAN_APPROVE, PLAN_DECLINE, PLAN_FEEDBACK } from '$lib/plan';
  import ModelPicker from '$lib/components/conversation/ModelPicker.svelte';
  import ContextRing from '$lib/components/conversation/ContextRing.svelte';
  import { screenshot, MAX_ATTACHMENTS, type Screenshot } from '$lib/attachments';
  import { INTERN_COMMANDS, slashSuggestions, SOURCE_LABEL } from '$lib/slash';
  import { runBuiltin } from '$lib/slash-actions';
  import { askConfirm } from '$lib/confirm.svelte';

  // Intern is app-wide. It only sees a project when one is attached here;
  // opening it from a thread attaches that thread's project.
  let { open, projects, threadProjectId, onClose, onPending, onAttention, onOpenTerminal }: { open: boolean; projects: Project[]; threadProjectId: string | null; onClose: () => void; onPending: (count: number) => void; onAttention?: () => void; onOpenTerminal?: () => void } = $props();
  let attachedId = $state<string | null>(null);
  let wasOpen = false;
  $effect(() => {
    const opening = open && !wasOpen;
    wasOpen = open;
    const threadProject = untrack(() => threadProjectId);
    if (opening && threadProject) attachedId = threadProject;
  });
  let pickerOpen = $state(false);
  const project = $derived(projects.find(candidate => candidate.id === attachedId) ?? null);
  let model = $state<SessionModel | null>(null);
  let loading = $state(true);
  let submitting = $state(false);
  let attaching = $state(false);
  let stopping = $state(false);
  let clearing = $state(false);
  let reloading = $state(false);
  let text = $state('');
  let error = $state('');
  let attachments = $state<Screenshot[]>([]);
  let plans = $state<InternPlan[]>([]);
  let answering = $state<Set<string>>(new Set());
  let fileInput: HTMLInputElement;
  let textarea: HTMLTextAreaElement;
  let disposed = false;
  let buffered: Record<string, unknown>[] = [];
  let bufferedBytes = 0;
  const busy = $derived(submitting || model?.view.status === 'active' || model?.view.status === 'waiting');
  const ready = $derived(model && !loading && !stopping && !clearing && model.view.status !== 'disconnected');
  let refreshGeneration = 0;
  let panel: HTMLDivElement;

  type Geometry = { x: number; y: number; width: number; height: number };
  type Edge = 'top' | 'right' | 'bottom' | 'left' | 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right';
  const edges: Edge[] = ['top', 'right', 'bottom', 'left', 'top-left', 'top-right', 'bottom-left', 'bottom-right'];
  const geometryKey = 'piInternGeometry';
  let viewportWidth = $state(typeof window === 'undefined' ? 1280 : window.innerWidth);
  let viewportHeight = $state(typeof window === 'undefined' ? 800 : window.innerHeight);
  let savedGeometry = $state<Geometry | null>(null);
  const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);
  const geometry = $derived.by(() => {
    const width = clamp(savedGeometry?.width ?? Math.min(620, viewportWidth - 48), Math.min(360, viewportWidth - 16), viewportWidth - 16);
    const height = clamp(savedGeometry?.height ?? Math.min(760, viewportHeight - 110), Math.min(320, viewportHeight - 16), viewportHeight - 16);
    return {
      width, height,
      x: clamp(savedGeometry?.x ?? viewportWidth - width - 20, 8, viewportWidth - width - 8),
      y: clamp(savedGeometry?.y ?? viewportHeight - height - 44, 8, viewportHeight - height - 8),
    };
  });
  const planPlacement = $derived.by(() => {
    const leftSpace = geometry.x - 24;
    const rightSpace = viewportWidth - geometry.x - geometry.width - 24;
    const width = Math.min(580, Math.max(leftSpace, rightSpace));
    if (width >= 380) {
      if (leftSpace >= rightSpace) return { x: geometry.x - width - 12, width, overlay: false };
      return { x: geometry.x + geometry.width + 12, width, overlay: false };
    }
    return { x: geometry.x, width: geometry.width, overlay: true };
  });
  let interaction = $state<{ kind: 'move' | Edge; startX: number; startY: number; initial: Geometry } | null>(null);

  function resize(initial: Geometry, edge: Edge, dx: number, dy: number): Geometry {
    let { x, y, width, height } = initial;
    const minWidth = Math.min(360, viewportWidth - 16);
    const minHeight = Math.min(320, viewportHeight - 16);
    if (edge.includes('left')) {
      x = clamp(initial.x + dx, 8, initial.x + initial.width - minWidth);
      width = initial.width + initial.x - x;
    } else if (edge.includes('right')) {
      width = clamp(initial.width + dx, minWidth, viewportWidth - initial.x - 8);
    }
    if (edge.includes('top')) {
      y = clamp(initial.y + dy, 8, initial.y + initial.height - minHeight);
      height = initial.height + initial.y - y;
    } else if (edge.includes('bottom')) {
      height = clamp(initial.height + dy, minHeight, viewportHeight - initial.y - 8);
    }
    return { x, y, width, height };
  }
  function startInteraction(event: PointerEvent, kind: 'move' | Edge) {
    if (event.button !== 0 || (kind === 'move' && (event.target as Element).closest('button'))) return;
    event.preventDefault();
    interaction = { kind, startX: event.clientX, startY: event.clientY, initial: geometry };
  }
  function moveInteraction(event: PointerEvent) {
    if (!interaction) return;
    const { kind, startX, startY, initial } = interaction;
    const dx = event.clientX - startX;
    const dy = event.clientY - startY;
    savedGeometry = kind === 'move'
      ? { ...initial, x: clamp(initial.x + dx, 8, viewportWidth - initial.width - 8), y: clamp(initial.y + dy, 8, viewportHeight - initial.height - 8) }
      : resize(initial, kind, dx, dy);
  }
  function endInteraction() {
    if (!interaction) return;
    interaction = null;
    localStorage.setItem(geometryKey, JSON.stringify(geometry));
  }
  function keyboardResize(event: KeyboardEvent, edge: Edge) {
    const step = event.shiftKey ? 40 : 12;
    const dx = event.key === 'ArrowRight' ? step : event.key === 'ArrowLeft' ? -step : 0;
    const dy = event.key === 'ArrowDown' ? step : event.key === 'ArrowUp' ? -step : 0;
    if ((!dx && !dy) || (dx && !edge.includes('left') && !edge.includes('right')) || (dy && !edge.includes('top') && !edge.includes('bottom'))) return;
    event.preventDefault();
    event.stopPropagation();
    savedGeometry = resize(geometry, edge, dx, dy);
    localStorage.setItem(geometryKey, JSON.stringify(geometry));
  }
  function resetGeometry() {
    savedGeometry = null;
    localStorage.removeItem(geometryKey);
  }
  function keyboardMove(event: KeyboardEvent) {
    if (event.target !== event.currentTarget) return;
    if (event.key === 'Home') { event.preventDefault(); resetGeometry(); return; }
    const step = event.shiftKey ? 40 : 12;
    const dx = event.key === 'ArrowRight' ? step : event.key === 'ArrowLeft' ? -step : 0;
    const dy = event.key === 'ArrowDown' ? step : event.key === 'ArrowUp' ? -step : 0;
    if (!dx && !dy) return;
    event.preventDefault();
    savedGeometry = { ...geometry, x: clamp(geometry.x + dx, 8, viewportWidth - geometry.width - 8), y: clamp(geometry.y + dy, 8, viewportHeight - geometry.height - 8) };
    localStorage.setItem(geometryKey, JSON.stringify(savedGeometry));
  }

  // `/` commands: πDesk's built-ins that make sense for Intern, then Pi's own.
  let cmdIndex = $state(0);
  let cmdDismissed = $state(false);
  const suggestions = $derived(cmdDismissed || !model ? [] : slashSuggestions(text, model.view.commands, INTERN_COMMANDS));
  $effect(() => { suggestions.length; cmdIndex = 0; });
  // Escape hides the menu until the text changes.
  $effect(() => { text; cmdDismissed = false; });
  function pickSuggestion(name: string) {
    text = `/${name} `;
    textarea?.focus();
  }
  function onKey(event: KeyboardEvent) {
    if (suggestions.length) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        cmdIndex = (cmdIndex + (event.key === 'ArrowDown' ? 1 : suggestions.length - 1)) % suggestions.length;
        return;
      }
      if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey && !event.isComposing)) {
        event.preventDefault();
        pickSuggestion(suggestions[Math.min(cmdIndex, suggestions.length - 1)].name);
        return;
      }
      // Close the menu, not the panel.
      if (event.key === 'Escape') { event.stopPropagation(); cmdDismissed = true; return; }
    }
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); void send(); }
  }

  async function refreshPlans() {
    const generation = ++refreshGeneration;
    try {
      const next = await api.internPlans();
      if (disposed || generation !== refreshGeneration) return;
      plans = next;
      onPending(next.filter(plan => !plan.executing).length);
    } catch (reason) { if (!disposed) error = String(reason); }
  }
  async function load() {
    loading = true;
    error = '';
    try {
      const snapshot = await api.internSnapshot();
      if (disposed) return;
      model = new SessionModel(snapshot);
      for (const frame of buffered) model.apply(frame);
      buffered = []; bufferedBytes = 0;
      await refreshPlans();
    } catch (reason) { if (!disposed) error = String(reason); }
    finally { if (!disposed) loading = false; }
  }
  function event(event: BackendEvent) {
    if (event.type === 'intern_changed') { void refreshPlans(); return; }
    if (!('threadId' in event) || event.threadId !== 'pidesk-intern') return;
    if (event.type === 'rpc') {
      if (model && !loading) model.apply(event.frame);
      else {
        bufferedBytes += JSON.stringify(event.frame).length;
        if (buffered.length >= 512 || bufferedBytes > 4 * 1024 * 1024) {
          error = 'Intern received too much output while opening. Stop and reopen it.';
          buffered = []; bufferedBytes = 0;
          void api.internStop();
        } else buffered.push(event.frame);
      }
    } else if (event.type === 'exited') {
      // A reload stops Pi on purpose and immediately starts a new one.
      if (event.expected && reloading) return;
      model?.setError(event.expected ? 'Intern stopped. Reopen to continue.' : 'Private Pi stopped unexpectedly. Reopen to continue.');
      void refreshPlans();
    }
  }
  onMount(() => {
    try {
      const stored = JSON.parse(localStorage.getItem(geometryKey) ?? 'null');
      if (stored && ['x', 'y', 'width', 'height'].every(key => typeof stored[key] === 'number' && Number.isFinite(stored[key]))) savedGeometry = stored;
    } catch { /* Ignore corrupt saved geometry. */ }
    let unlisten: (() => void) | undefined;
    void onBackendEvent(event).then(listener => {
      if (disposed) { listener(); return; }
      unlisten = listener;
      void load();
    }).catch(reason => { if (!disposed) { error = `Could not connect Intern events: ${reason}`; loading = false; } });
    return () => { disposed = true; unlisten?.(); };
  });
  $effect(() => { if (open) textarea?.focus(); });

  async function addFiles(files: File[]) {
    if (attaching) return;
    attaching = true;
    try {
      if (attachments.length + files.length > MAX_ATTACHMENTS) throw new Error('Attach at most four screenshots.');
      const added = await Promise.all(files.map(screenshot));
      if (!disposed) attachments = [...attachments, ...added];
    } catch (reason) { error = reason instanceof Error ? reason.message : String(reason); }
    finally { attaching = false; }
  }
  /** Built-ins run in πDesk; any other `/command` goes to Pi as typed. */
  async function runCommand(message: string): Promise<boolean> {
    if (!model || attachments.length || !message.startsWith('/')) return false;
    return runBuiltin(message, {
      threadId: 'pidesk-intern',
      session: model,
      setModel: selectModel,
      setEffort: async level => {
        const state = await api.setThreadEffort('pidesk-intern', level);
        if (model) model.view.effort = state.thinkingLevel ?? level;
      },
      reload: reloadPi,
      startNew: clear,
      openModelPicker: () => panel?.querySelector<HTMLButtonElement>('button[aria-label="Select model"]')?.click(),
      login: onOpenTerminal,
    });
  }
  async function send() {
    if (!ready || busy || attaching || (!text.trim() && !attachments.length)) return;
    submitting = true; error = '';
    const message = `${text.trim() || 'Please inspect these screenshots.'}${attachments.length ? `\n\nAttached screenshots: ${attachments.map(file => file.name).join(', ')}` : ''}`;
    try {
      if (!(await runCommand(message))) await api.internPrompt(message, project?.id, attachments.map(({ data, mimeType }) => ({ data, mimeType })));
      text = ''; attachments = [];
    } catch (reason) { error = String(reason); }
    finally { submitting = false; }
  }
  async function answer(plan: InternPlan, approved: boolean) {
    if (answering.has(plan.id)) return;
    answering = new Set([...answering, plan.id]);
    try { await api.internApprove(plan.id, approved); await refreshPlans(); }
    catch (reason) { error = String(reason); }
    finally { const next = new Set(answering); next.delete(plan.id); answering = next; }
  }
  async function stop() {
    stopping = true;
    try { await api.internStop(); model?.setError('Intern stopped. Reopen to continue.'); await refreshPlans(); }
    catch (reason) { error = String(reason); }
    finally { stopping = false; }
  }
  /** Fresh conversation in the same Pi; the attached project and draft stay. */
  /** Restart Intern's Pi so new extensions, MCP servers and settings load.
   *  The conversation resumes; pending approvals can't carry over. */
  async function reloadPi() {
    if (reloading || loading) return;
    if (busy && !(await askConfirm({ title: 'Reload Pi now?', subject: 'Pi Intern', detail: 'Pi Intern is working. Reloading stops the current run.', confirmLabel: 'Stop and reload' }))) return;
    reloading = true;
    error = '';
    try {
      await api.internStop();
      await load();
      model?.notify('info', 'Reloaded Pi: new extensions, MCP servers and settings are loaded.');
    } catch (reason) {
      if (!disposed) error = String(reason);
    } finally {
      // Let the stop's exit event arrive before listening for real exits again.
      setTimeout(() => { reloading = false; }, 1000);
    }
  }

  async function clear() {
    if (clearing || loading) return;
    if (busy && !(await askConfirm({ title: 'Clear the conversation?', subject: 'Pi Intern', detail: 'Pi Intern is working. Clearing stops the current run.', confirmLabel: 'Stop and clear' }))) return;
    clearing = true; error = '';
    try {
      const snapshot = await api.internClear();
      if (disposed) return;
      model = new SessionModel(snapshot);
      await refreshPlans();
      textarea?.focus();
    } catch (reason) { if (!disposed) error = String(reason); }
    finally { if (!disposed) clearing = false; }
  }
  // A Plan → Auto approval opens beside the panel, showing Intern if hidden.
  const planRequest = $derived(model?.view.pendingRequests.find(request => planOf(request) !== null));
  const otherRequests = $derived(model?.view.pendingRequests.filter(request => planOf(request) === null) ?? []);
  let planHidden = $state(false);
  let shownPlanId: string | undefined;
  $effect(() => {
    const id = planRequest?.id;
    if (!id || id === shownPlanId) return;
    shownPlanId = id;
    planHidden = false;
    untrack(() => onAttention?.());
  });
  async function answerPlan(choice: 'approve' | 'decline' | 'feedback', feedback?: string) {
    const request = planRequest;
    if (!request) return;
    try {
      if (choice === 'feedback' && feedback) await api.sendPrompt('pidesk-intern', feedback, 'steer');
      await respond(request.id, { value: choice === 'approve' ? PLAN_APPROVE : choice === 'feedback' ? PLAN_FEEDBACK : PLAN_DECLINE });
    } catch (reason) { error = String(reason); }
  }
  /** Pi's own prompts other than plan approvals. */
  async function respond(requestId: string, response: UiResponse) {
    try { await api.respondUi('pidesk-intern', requestId, response); model?.dismissRequest(requestId); }
    catch (reason) { error = String(reason); }
  }
  async function selectModel(value: string) {
    const slash = value.indexOf('/');
    if (slash < 0 || !model) return;
    try {
      const state = await api.setThreadModel('pidesk-intern', value.slice(0, slash), value.slice(slash + 1));
      if (state.model) model.view.model = state.model;
    } catch (reason) { error = String(reason); }
  }
</script>

<!-- Non-modal: the project and other threads remain usable while Intern works. -->
<svelte:window bind:innerWidth={viewportWidth} bind:innerHeight={viewportHeight} onpointermove={moveInteraction} onpointerup={endInteraction} onpointercancel={endInteraction} onclick={event => { if (pickerOpen && !(event.target as Element | null)?.closest('.picker')) pickerOpen = false; }} />
<div bind:this={panel} class="intern" role="dialog" aria-modal="false" tabindex="-1" class:hidden={!open} class:moving={interaction?.kind === 'move'} aria-label="Pi Intern" style={`left:${geometry.x}px;top:${geometry.y}px;width:${geometry.width}px;height:${geometry.height}px`} onkeydown={event => { event.stopPropagation(); if (event.key === 'Escape') { if (pickerOpen) pickerOpen = false; else onClose(); } }}>
  <header><div class="drag-handle" role="button" tabindex="0" aria-label="Move Pi Intern (arrow keys; Home resets position and size)" title="Drag to move · double-click to reset size and position" onpointerdown={event => startInteraction(event, 'move')} ondblclick={resetGeometry} onkeydown={keyboardMove}><span class="title"><Bot size={18}/> Pi Intern</span><span class="scope" title={project?.path ?? 'Not attached to a project'}>{project ? project.displayName : 'All of πDesk'}</span></div><button title="Reload Pi: restart Intern's Pi to load new extensions, MCP servers and settings" aria-label="Reload Pi Intern" disabled={loading || clearing || stopping || reloading} onclick={() => void reloadPi()}><RefreshCw size={15} class={reloading ? 'spin' : ''}/></button><button title="New conversation (clears this chat, keeps Pi running)" aria-label="Clear Pi Intern conversation" disabled={loading || clearing || stopping || !model} onclick={() => void clear()}><Eraser size={15}/></button><button title="Hide Intern (work continues)" aria-label="Hide Pi Intern" onclick={onClose}><X size={16}/></button></header>
  <div class="policy"><ShieldCheck size={13}/> Plan mode · read-only until you approve a plan · πDesk actions need approval</div>
  {#if error || model?.view.error}<div class="error" role="alert">{error || model?.view.error}<button disabled={loading || busy} onclick={() => void load()}><RefreshCw size={12}/> Reopen</button></div>{/if}
  {#if loading}<div class="empty" role="status">Opening Pi Intern…</div>
  {:else if model}<Transcript items={model.view.items} status={model.view.status}/>
  {:else}<div class="empty">Pi Intern needs a working private Pi installation and a connected provider. Check Settings to repair the runtime or sign in.</div>{/if}
  {#if planRequest && planHidden}
    <button class="plan-ready" onclick={() => planHidden = false}><ClipboardList size={14}/><span>A plan is waiting for your approval</span><span class="go">Review plan</span></button>
  {/if}
  {#if otherRequests.length}
    <div class="plans" aria-live="polite">
      {#each otherRequests as request (request.id)}<RequestCard {request} onRespond={respond}/>{/each}
    </div>
  {/if}
  {#if plans.length}
    <div class="plans" aria-label="Intern approval plans">
      {#each plans as plan (plan.id)}
        <section class="plan" aria-label={`Approval: ${plan.summary}`}>
          <strong>{plan.summary}</strong><small>{plan.threadTitle} · {plan.projectPath ?? 'Private Pi'}</small>
          <p>Approve only these file changes and app actions. Further actions need a new approval. General shell commands are not available to Intern.</p>
          {#each plan.actions as action, i}
            <details open><summary>{i + 1}. {String(action.kind).replaceAll('_', ' ')}{action.path ? ` · ${action.path}` : ''}</summary>
              <pre>{JSON.stringify({ action, target: plan.details[i] }, null, 2)}</pre>
            </details>
          {/each}
          <div class="plan-actions">
            <button disabled={plan.executing || answering.has(plan.id)} onclick={() => void answer(plan, false)}>Reject plan</button>
            <button class="approve" disabled={plan.executing || answering.has(plan.id)} onclick={() => void answer(plan, true)}>{plan.executing ? 'Executing approved plan…' : 'Approve exact plan'}</button>
          </div>
        </section>
      {/each}
    </div>
  {/if}
  <div class="composer">
    {#if project}<div class="attached"><Folder size={12}/><span title={project.path}>{project.displayName}</span><button aria-label={`Detach ${project.displayName}`} title="Detach project" onclick={() => attachedId = null}><X size={11}/></button></div>{/if}
    {#if suggestions.length}
      <div class="suggest" id="intern-command-suggestions" role="listbox" aria-label="Commands">
        {#each suggestions as command, i (command.name)}
          <button id={`intern-command-${i}`} type="button" role="option" aria-selected={i === cmdIndex} class="sug" class:active={i === cmdIndex}
            onmousedown={event => { event.preventDefault(); pickSuggestion(command.name); }}>
            <span class="sug-name">/{command.name}{#if command.args}<span class="sug-args"> {command.args}</span>{/if}</span>
            {#if command.description}<span class="sug-desc">{command.description}</span>{/if}
            <span class="sug-source" data-source={command.source}>{SOURCE_LABEL[command.source]}</span>
          </button>
        {/each}
      </div>
    {/if}
    {#if attachments.length}<div class="attachments">{#each attachments as file, index}<div><img src={`data:${file.mimeType};base64,${file.data}`} alt={file.name}/><button aria-label={`Remove ${file.name}`} onclick={() => attachments = attachments.filter((_, i) => i !== index)}><X size={12}/></button></div>{/each}</div>{/if}
    <textarea bind:this={textarea} bind:value={text} aria-label="Message Pi Intern" placeholder={project ? `Ask about ${project.displayName}, or attach a screenshot…` : 'Ask about Pi or πDesk, attach a project or a screenshot…'} rows="2" maxlength="32000" disabled={!ready || busy}
      onpaste={event => { const files = Array.from(event.clipboardData?.files ?? []); if (files.length) { event.preventDefault(); void addFiles(files); } }}
      aria-controls={suggestions.length ? 'intern-command-suggestions' : undefined} aria-activedescendant={suggestions.length ? `intern-command-${cmdIndex}` : undefined}
      onkeydown={onKey}></textarea>
    <input bind:this={fileInput} type="file" accept="image/png,image/jpeg,image/webp" multiple aria-label="Attach screenshots" onchange={event => { void addFiles(Array.from(event.currentTarget.files ?? [])); event.currentTarget.value = ''; }} hidden />
    <div class="toolbar">
      <button aria-label="Attach screenshots" title="Attach screenshots (or paste)" disabled={!ready || busy || attaching || attachments.length>=MAX_ATTACHMENTS || model?.view.model?.images===false} onclick={() => fileInput.click()}><Paperclip size={15}/></button>
      <div class="picker">
        <button aria-label="Attach project" title="Attach a project" aria-haspopup="menu" aria-expanded={pickerOpen} disabled={busy} onclick={() => pickerOpen = !pickerOpen}><FolderPlus size={15}/></button>
        {#if pickerOpen}
          <div class="menu" role="menu" aria-label="Attach project">
            <button role="menuitemradio" aria-checked={!project} onclick={() => { attachedId = null; pickerOpen = false; }}><Globe size={13}/><span>No project (global)</span>{#if !project}<Check size={13}/>{/if}</button>
            {#each projects as candidate (candidate.id)}
              <button role="menuitemradio" aria-checked={project?.id === candidate.id} title={candidate.path} onclick={() => { attachedId = candidate.id; pickerOpen = false; }}><Folder size={13}/><span>{candidate.displayName}</span>{#if project?.id === candidate.id}<Check size={13}/>{/if}</button>
            {:else}
              <p>No projects yet. Add one from the sidebar.</p>
            {/each}
          </div>
        {/if}
      </div>
      {#if model?.view.models.length}<ModelPicker models={model.view.models} current={model.view.model ?? null} onSelect={selectModel}/>{/if}
      <span class="grow"></span>
      <ContextRing usage={model?.view.contextUsage} />
      <button aria-label="Stop Intern" title="Stop Intern (threads it started keep running)" disabled={stopping || loading} onclick={() => void stop()}><Square size={13}/></button>
      <button class="send" aria-label="Send to Pi Intern" disabled={!ready || busy || attaching || (!text.trim() && !attachments.length)} onclick={() => void send()}><ArrowUp size={16}/></button>
    </div>
    <small class="privacy">Screenshots and selected files go to your configured model provider. Review them for secrets.</small>
  </div>
  {#each edges as edge}
    <div class={`resize-${edge}`} role="button" tabindex="0" aria-label={`Resize Pi Intern ${edge.replace('-', ' ')}`} onpointerdown={event => { event.stopPropagation(); startInteraction(event, edge); }} onkeydown={event => keyboardResize(event, edge)}></div>
  {/each}
</div>
{#if open && planRequest && !planHidden}
  <div class="plan-sheet" class:overlay={planPlacement.overlay} style={`left:${planPlacement.x}px;top:${geometry.y}px;width:${planPlacement.width}px;height:${geometry.height}px`}>
    <PlanReview plan={planOf(planRequest) ?? ''} source="Pi Intern" onApprove={() => answerPlan('approve')} onDecline={() => answerPlan('decline')} onFeedback={text => answerPlan('feedback', text)} onClose={() => planHidden = true} />
  </div>
{/if}

<style>
  .intern { position:fixed; z-index:25; display:flex; flex-direction:column; background:var(--bg); border:1px solid var(--line-strong); box-shadow:var(--shadow); border-radius:var(--radius-lg); overflow:hidden; }
  .intern.hidden { display:none; }
  header { display:flex; align-items:center; gap:9px; padding:12px 14px; background:var(--elevated); border-bottom:1px solid var(--line); }
  .drag-handle { display:flex; align-items:center; flex:1; min-width:0; align-self:stretch; cursor:grab; touch-action:none; user-select:none; }
  .intern.moving .drag-handle { cursor:grabbing; }
  .drag-handle:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  .title { display:flex; align-items:center; gap:8px; font-size:14px; font-weight:600; }
  .title :global(svg) { color:var(--accent); }
  .scope { margin-left:auto; color:var(--muted); font-size:11px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; max-width:200px; }
  button { display:inline-flex; align-items:center; justify-content:center; gap:5px; border:1px solid var(--line); border-radius:6px; padding:6px 8px; background:var(--surface); color:var(--text); font:12px var(--font); }
  button:disabled { opacity:.5; }
  .policy { display:flex; align-items:center; gap:6px; padding:7px 14px; color:var(--muted); background:var(--surface); font-size:11px; }
  .empty { flex:1; padding:24px; font-size:13px; color:var(--muted); }
  .error { padding:10px 14px; font-size:12px; background:var(--bad-bg); color:var(--bad); overflow-wrap:anywhere; }
  .error button { margin:6px; }
  .plans { max-height:42%; flex:none; overflow:auto; border-top:1px solid var(--line); padding:10px 12px; }
  .plan { background:var(--surface); border:1px solid var(--line-strong); padding:10px; border-radius:8px; margin-bottom:8px; }
  .plan strong { display:block; font-size:13px; }
  .plan small { display:block; margin:4px 0; font-size:11px; color:var(--muted); overflow-wrap:anywhere; }
  .plan p { font-size:11px; color:var(--muted); margin:6px 0; }
  details { font-size:12px; margin:8px 0; }
  summary { cursor:pointer; overflow-wrap:anywhere; }
  pre { font:11px/1.5 var(--mono); white-space:pre-wrap; overflow-wrap:anywhere; max-height:240px; overflow:auto; padding:8px; background:var(--bg); border-radius:6px; }
  .plan-actions { display:flex; justify-content:flex-end; gap:7px; }
  .approve,.send { background:var(--accent-strong); color:var(--on-accent); }
  .composer { position:relative; flex:none; padding:10px 12px; border-top:1px solid var(--line); }
  .suggest { position:absolute; left:12px; right:12px; bottom:calc(100% - 4px); z-index:3; display:flex; flex-direction:column; max-height:260px; overflow-y:auto; padding:4px; border:1px solid var(--line); border-radius:var(--radius-lg); background:var(--elevated); box-shadow:var(--shadow); animation:ui-pop .12s var(--ease); }
  .sug { display:flex; gap:10px; align-items:baseline; justify-content:flex-start; padding:6px 10px; border:0; border-radius:var(--radius-sm); background:transparent; color:var(--text); font-size:12.5px; text-align:left; }
  .sug.active, .sug:hover { background:var(--accent-bg); }
  .sug-name { flex:none; color:var(--accent); font:12px var(--mono); }
  .sug-args { color:var(--subtle); }
  .sug-desc { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--muted); font-size:12px; }
  .sug-source { flex:none; align-self:center; height:17px; padding:0 6px; border-radius:5px; background:var(--surface-2); color:var(--subtle); font-size:10.5px; font-weight:600; line-height:17px; }
  .sug-source[data-source='pidesk'] { background:var(--accent-bg); color:var(--accent); }
  textarea { display:block; resize:vertical; width:100%; max-height:140px; min-height:56px; border:1px solid var(--line-strong); border-radius:8px; background:var(--surface); color:var(--text); padding:9px; font:13px/1.5 var(--font); }
  .toolbar { display:flex; gap:7px; align-items:center; margin-top:8px; }
  .grow { flex:1; }
  .privacy { display:block; font-size:10px; color:var(--subtle); margin-top:7px; }
  .attached { display:inline-flex; align-items:center; gap:5px; max-width:100%; margin-bottom:8px; padding:3px 4px 3px 8px; border-radius:6px; background:var(--accent-bg); color:var(--accent); font-size:11.5px; }
  .attached span { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .attached button { border:0; padding:2px; background:none; color:inherit; }
  .picker { position:relative; }
  .menu { position:absolute; left:0; bottom:calc(100% + 6px); z-index:2; width:260px; max-height:280px; overflow:auto; padding:4px; background:var(--elevated); border:1px solid var(--line-strong); border-radius:8px; box-shadow:var(--shadow); }
  .menu button { width:100%; justify-content:flex-start; border:0; background:none; padding:6px 8px; }
  .menu button:hover { background:var(--surface); }
  .menu span { flex:1; text-align:left; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .menu p { margin:6px 8px; font-size:11.5px; color:var(--muted); }
  .plan-sheet { position:fixed; z-index:25; background:var(--panel); border:1px solid var(--line-strong); box-shadow:var(--shadow); border-radius:var(--radius-lg); overflow:hidden; animation:ui-rise .2s var(--ease); }
  .plan-sheet.overlay { z-index:26; }
  [class^="resize-"] { position:absolute; z-index:2; touch-action:none; }
  .resize-top, .resize-bottom { left:12px; right:12px; height:8px; cursor:ns-resize; }
  .resize-top { top:0; } .resize-bottom { bottom:0; }
  .resize-left, .resize-right { top:12px; bottom:12px; width:8px; cursor:ew-resize; }
  .resize-left { left:0; } .resize-right { right:0; }
  .resize-top-left, .resize-top-right, .resize-bottom-left, .resize-bottom-right { width:16px; height:16px; }
  .resize-top-left { top:0; left:0; cursor:nwse-resize; }
  .resize-top-right { top:0; right:0; cursor:nesw-resize; }
  .resize-bottom-left { bottom:0; left:0; cursor:nesw-resize; }
  .resize-bottom-right { bottom:0; right:0; cursor:nwse-resize; }
  .resize-bottom-right::after { content:''; position:absolute; bottom:4px; right:4px; width:6px; height:6px; border-right:2px solid var(--subtle); border-bottom:2px solid var(--subtle); }
  [class^="resize-"]:focus-visible { outline:2px solid var(--accent); outline-offset:-2px; }
  .plan-ready { display:flex; align-items:center; gap:8px; margin:8px 12px 0; padding:9px 12px; border:1px solid var(--line-strong); border-radius:8px; background:var(--warn-bg); color:var(--text); font:500 12.5px var(--font); justify-content:flex-start; }
  .plan-ready :global(svg) { color:var(--warn); }
  .plan-ready .go { margin-left:auto; color:var(--accent); font-weight:600; }
  .attachments { display:flex; gap:8px; padding-bottom:8px; }
  .attachments > div { position:relative; }
  .attachments img { height:64px; width:88px; object-fit:contain; background:var(--surface); border-radius:5px; }
  .attachments button { position:absolute; right:0; top:0; padding:2px; }
</style>
