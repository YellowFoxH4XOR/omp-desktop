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
  import { screenshot, MAX_ATTACHMENTS, type Screenshot } from './attachments';

  // Intern is app-wide. It only sees a project when one is attached here;
  // opening it from a thread attaches that thread's project.
  let { open, projects, threadProjectId, onClose, onPending, onAttention }: { open: boolean; projects: Project[]; threadProjectId: string | null; onClose: () => void; onPending: (count: number) => void; onAttention?: () => void } = $props();
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
      model?.setError(event.expected ? 'Intern stopped. Reopen to continue.' : 'Private Pi stopped unexpectedly. Reopen to continue.');
      void refreshPlans();
    }
  }
  onMount(() => {
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
  async function send() {
    if (!ready || busy || attaching || (!text.trim() && !attachments.length)) return;
    submitting = true; error = '';
    const message = `${text.trim() || 'Please inspect these screenshots.'}${attachments.length ? `\n\nAttached screenshots: ${attachments.map(file => file.name).join(', ')}` : ''}`;
    try {
      await api.internPrompt(message, project?.id, attachments.map(({ data, mimeType }) => ({ data, mimeType })));
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
  async function clear() {
    if (clearing || loading) return;
    if (busy && !window.confirm('Pi Intern is working. Stop it and clear the conversation?')) return;
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
<svelte:window onclick={event => { if (pickerOpen && !(event.target as Element | null)?.closest('.picker')) pickerOpen = false; }} />
<div class="intern" role="dialog" aria-modal="false" tabindex="-1" class:hidden={!open} aria-label="Pi Intern" onkeydown={event => { event.stopPropagation(); if (event.key === 'Escape') { if (pickerOpen) pickerOpen = false; else onClose(); } }}>
  <header><span class="title"><Bot size={18}/> Pi Intern</span><span class="scope" title={project?.path ?? 'Not attached to a project'}>{project ? project.displayName : 'All of πDesk'}</span><button title="New conversation (clears this chat, keeps Pi running)" aria-label="Clear Pi Intern conversation" disabled={loading || clearing || stopping || !model} onclick={() => void clear()}><Eraser size={15}/></button><button title="Hide Intern (work continues)" aria-label="Hide Pi Intern" onclick={onClose}><X size={16}/></button></header>
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
    {#if attachments.length}<div class="attachments">{#each attachments as file, index}<div><img src={`data:${file.mimeType};base64,${file.data}`} alt={file.name}/><button aria-label={`Remove ${file.name}`} onclick={() => attachments = attachments.filter((_, i) => i !== index)}><X size={12}/></button></div>{/each}</div>{/if}
    <textarea bind:this={textarea} bind:value={text} aria-label="Message Pi Intern" placeholder={project ? `Ask about ${project.displayName}, or attach a screenshot…` : 'Ask about Pi or πDesk, attach a project or a screenshot…'} rows="2" maxlength="32000" disabled={!ready || busy}
      onpaste={event => { const files = Array.from(event.clipboardData?.files ?? []); if (files.length) { event.preventDefault(); void addFiles(files); } }}
      onkeydown={event => { if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); void send(); } }}></textarea>
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
      <button aria-label="Stop Intern" title="Stop Intern (threads it started keep running)" disabled={stopping || loading} onclick={() => void stop()}><Square size={13}/></button>
      <button class="send" aria-label="Send to Pi Intern" disabled={!ready || busy || attaching || (!text.trim() && !attachments.length)} onclick={() => void send()}><ArrowUp size={16}/></button>
    </div>
    <small class="privacy">Screenshots and selected files go to your configured model provider. Review them for secrets.</small>
  </div>
</div>
{#if open && planRequest && !planHidden}
  <div class="plan-sheet">
    <PlanReview plan={planOf(planRequest) ?? ''} source="Pi Intern" onApprove={() => answerPlan('approve')} onDecline={() => answerPlan('decline')} onFeedback={text => answerPlan('feedback', text)} onClose={() => planHidden = true} />
  </div>
{/if}

<style>
  .intern { position:fixed; right:20px; bottom:44px; z-index:25; width:min(620px,calc(100vw - 48px)); height:min(760px,calc(100vh - 110px)); display:flex; flex-direction:column; background:var(--bg); border:1px solid var(--line-strong); box-shadow:var(--shadow); border-radius:var(--radius-lg); overflow:hidden; }
  .intern.hidden { display:none; }
  header { display:flex; align-items:center; gap:9px; padding:12px 14px; background:var(--elevated); border-bottom:1px solid var(--line); }
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
  .composer { flex:none; padding:10px 12px; border-top:1px solid var(--line); }
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
  .plan-sheet { position:fixed; right:calc(32px + min(620px, calc(100vw - 48px))); bottom:44px; z-index:25; width:min(580px, calc(100vw - 700px)); height:min(760px,calc(100vh - 110px)); background:var(--panel); border:1px solid var(--line-strong); box-shadow:var(--shadow); border-radius:var(--radius-lg); overflow:hidden; animation:ui-rise .2s var(--ease); }
  /* Too narrow to sit beside Intern: cover it instead. */
  @media (max-width: 1080px) { .plan-sheet { right:20px; z-index:26; width:min(620px,calc(100vw - 48px)); } }
  .plan-ready { display:flex; align-items:center; gap:8px; margin:8px 12px 0; padding:9px 12px; border:1px solid var(--line-strong); border-radius:8px; background:var(--warn-bg); color:var(--text); font:500 12.5px var(--font); justify-content:flex-start; }
  .plan-ready :global(svg) { color:var(--warn); }
  .plan-ready .go { margin-left:auto; color:var(--accent); font-weight:600; }
  .attachments { display:flex; gap:8px; padding-bottom:8px; }
  .attachments > div { position:relative; }
  .attachments img { height:64px; width:88px; object-fit:contain; background:var(--surface); border-radius:5px; }
  .attachments button { position:absolute; right:0; top:0; padding:2px; }
</style>
