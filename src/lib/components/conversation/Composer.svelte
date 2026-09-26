<script lang="ts" module>
  /** Per-thread file lists for `@` mentions, reused for a short while. */
  const fileCache = new Map<string, { paths: string[]; truncated: boolean; at: number }>();
  const FILE_CACHE_MS = 30_000;
</script>

<script lang="ts">
  import { ArrowUp, Brain, ChevronDown, ChevronRight, ClipboardList, CornerDownLeft, Cpu, FileText, Folder, Square, Zap } from '@lucide/svelte';
  import { tick } from 'svelte';
  import { api } from '../../api';
  import { mentionAt, rankPaths, withFolders } from '../../fuzzy';
  import type { ContextUsage, ModelInfo, ThreadMode, ThreadStatus } from '../../types';
  import ModelPicker from './ModelPicker.svelte';
  import { slide } from 'svelte/transition';
  import { backOut } from 'svelte/easing';
  import { planFacts, type PlanActions } from '../../plan';
  import { slashSuggestions, SOURCE_LABEL } from '../../slash';
  import ContextRing from './ContextRing.svelte';
  import { modelKey } from './model-utils';

  type SendMode = 'prompt' | 'steer' | 'follow_up';

  interface Props {
    threadId: string;
    status: ThreadStatus;
    commands?: Array<{ name: string; description?: string; source?: 'extension' | 'skill' | 'prompt' }>;
    model?: ModelInfo;
    models?: ModelInfo[];
    effort?: string;
    levels?: string[];
    contextUsage?: ContextUsage;
    onSend: (message: string, mode: SendMode) => void | Promise<void>;
    onAbort: () => void | Promise<void>;
    onSetModel?: (value: string) => void | Promise<void>;
    /** `provider/id` of the default model for new threads. */
    defaultModelKey?: string | null;
    onMakeDefault?: (model: ModelInfo) => void | Promise<void>;
    onSetEffort?: (level: string) => void | Promise<void>;
    /** Plan/Auto for this thread; the toggle is hidden without `onSetMode`. */
    agentMode?: ThreadMode;
    onSetMode?: (mode: ThreadMode) => void | Promise<void>;
    /** A pending plan turns the top of the composer into an approval strip. */
    approval?: PlanActions;
  }

  let {
    threadId,
    status,
    commands = [],
    model,
    models = [],
    effort,
    levels = [],
    contextUsage,
    onSend,
    onAbort,
    onSetModel,
    defaultModelKey = null,
    onMakeDefault,
    onSetEffort,
    agentMode = 'auto',
    onSetMode,
    approval,
  }: Props = $props();

  const reduceMotion = typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches;
  const facts = $derived(approval ? planFacts(approval.plan) : null);
  let feedbackOpen = $state(false);
  let feedback = $state('');
  let answering = $state(false);
  $effect(() => { if (!approval) { feedbackOpen = false; feedback = ''; } });
  async function answer(run: () => void | Promise<void>) {
    if (answering) return;
    answering = true;
    try { await run(); } finally { answering = false; }
  }
  function sendFeedback() {
    const note = feedback.trim();
    if (note && approval) void answer(() => approval.onFeedback(note));
  }

  // Session state carries a slim model; the catalogue entry has context and traits.
  const currentModel = $derived(model ? (models.find((entry) => modelKey(entry) === modelKey(model)) ?? model) : null);
  function capitalize(value: string): string {
    return value ? value[0].toUpperCase() + value.slice(1) : value;
  }

  let text = $state('');
  let submitting = $state(false);
  let area = $state<HTMLTextAreaElement>();
  // Enter sends and Shift+Enter adds a line, unless switched to ⌘Enter.
  const ENTER_SEND_KEY = 'composerEnterSends';
  let enterSend = $state((() => { try { return localStorage.getItem(ENTER_SEND_KEY) !== 'false'; } catch { return true; } })());
  function toggleEnterSend() {
    enterSend = !enterSend;
    try { localStorage.setItem(ENTER_SEND_KEY, String(enterSend)); } catch { /* per-device preference only */ }
  }
  let streamMode = $state<'steer' | 'follow_up'>('steer');
  let modeOpen = $state(false);
  let cmdIndex = $state(0);
  const commandListId = 'composer-command-suggestions';
  const optionId = (index: number) => `${commandListId}-option-${index}`;
  let cmdDismissed = $state(false);
  let submitGeneration = 0;

  const streaming = $derived(status === 'active');
  const disabled = $derived(status === 'disconnected');
  const canSend = $derived(text.trim().length > 0 && !disabled && !submitting);

  const suggestions = $derived(cmdDismissed ? [] : slashSuggestions(text, commands));

  // Focus the composer on mount and whenever a different thread is shown,
  // unless the user is typing somewhere else (e.g. renaming that thread).
  $effect(() => {
    threadId;
    submitGeneration += 1;
    submitting = false;
    text = '';
    cmdDismissed = false;
    const focused = document.activeElement;
    const typingElsewhere = focused instanceof HTMLElement && focused !== area
      && (focused.isContentEditable || (focused instanceof HTMLInputElement && !['button', 'checkbox', 'radio'].includes(focused.type)) || focused instanceof HTMLTextAreaElement);
    if (!typingElsewhere) area?.focus();
  });

  // Auto-grow the textarea up to a cap.
  $effect(() => {
    text;
    if (!area) return;
    area.style.height = 'auto';
    area.style.height = `${Math.min(area.scrollHeight, 180)}px`;
  });

  // Keep the highlighted suggestion in range as the list changes.
  $effect(() => {
    suggestions.length;
    cmdIndex = 0;
  });

  $effect(() => {
    if (suggestions.length === 0) return;
    document.getElementById(optionId(cmdIndex))?.scrollIntoView({ block: 'nearest' });
  });

  async function submit() {
    const message = text.trim();
    if (!message || disabled || submitting) return;
    const operation = ++submitGeneration;
    const originThreadId = threadId;
    submitting = true;
    try {
      await onSend(message, streaming ? streamMode : 'prompt');
      if (operation === submitGeneration && threadId === originThreadId && text.trim() === message) text = '';
    } catch {
      if (operation === submitGeneration && threadId === originThreadId) area?.focus();
    } finally {
      if (operation === submitGeneration && threadId === originThreadId) submitting = false;
    }
  }

  function pickSuggestion(name: string) {
    text = `/${name} `;
    cmdDismissed = false;
    area?.focus();
  }

  // ---- @ file mentions: files anywhere under the thread's folder ----
  let caret = $state(0);
  let mentionIndex = $state(0);
  let mentionDismissedAt = $state<number | null>(null);
  let filePaths = $state<string[]>([]);
  let filesTruncated = $state(false);
  let filesLoading = $state(false);
  const mentionListId = 'composer-file-suggestions';
  const mentionOptionId = (index: number) => `${mentionListId}-option-${index}`;
  const mention = $derived.by(() => {
    const found = mentionAt(text, caret);
    return found && found.start !== mentionDismissedAt ? found : null;
  });
  // Inside a folder (`@src/`) show more of its contents; otherwise the best 10.
  const mentionResults = $derived(mention ? rankPaths(filePaths, mention.query, mention.query.endsWith('/') ? 60 : 10) : []);
  const insideFolder = $derived(mention?.query.endsWith('/') ? mention.query : null);
  function syncCaret() {
    caret = area?.selectionStart ?? text.length;
  }
  async function loadFiles(id: string) {
    const cached = fileCache.get(id);
    if (cached) { filePaths = cached.paths; filesTruncated = cached.truncated; }
    if (cached && Date.now() - cached.at < FILE_CACHE_MS) return;
    filesLoading = !cached;
    try {
      const listing = await api.listThreadFiles(id);
      const entry = { paths: withFolders(listing.files), truncated: listing.truncated, at: Date.now() };
      fileCache.set(id, entry);
      if (id === threadId) { filePaths = entry.paths; filesTruncated = entry.truncated; }
    } catch {
      /* No listing (e.g. the folder is gone): mentions just show nothing. */
    } finally {
      filesLoading = false;
    }
  }
  $effect(() => {
    if (mention && threadId) void loadFiles(threadId);
  });
  $effect(() => {
    void mention?.query;
    mentionIndex = 0;
  });
  $effect(() => {
    if (mentionResults.length) document.getElementById(mentionOptionId(mentionIndex))?.scrollIntoView({ block: 'nearest' });
  });
  /** Insert `@path`. With `open`, a folder stays in progress so the list shows what's inside it. */
  async function pickMention(path: string, open = false) {
    if (!mention) return;
    const drill = open && path.endsWith('/');
    const before = text.slice(0, mention.start);
    const after = text.slice(caret).replace(/^\S*/, '');
    const inserted = drill ? `@${path}` : `@${path}${after.startsWith(' ') ? '' : ' '}`;
    text = before + inserted + after;
    const next = before.length + inserted.length;
    await tick();
    area?.focus();
    area?.setSelectionRange(next, next);
    caret = next;
  }
  function splitPath(path: string): { name: string; dir: string } {
    const trimmed = path.endsWith('/') ? path.slice(0, -1) : path;
    const at = trimmed.lastIndexOf('/');
    return { name: trimmed.slice(at + 1) + (path.endsWith('/') ? '/' : ''), dir: at >= 0 ? trimmed.slice(0, at + 1) : '' };
  }

  function onKeydown(event: KeyboardEvent) {
    // IME commits send keydown with isComposing/keyCode 229: never submit or
    // pick a suggestion mid-composition; the post-commit Enter works normally.
    if (event.isComposing || event.keyCode === 229) return;
    if (approval && event.metaKey && event.key === 'Enter' && !text.trim()) {
      event.preventDefault();
      void answer(approval.onApprove);
      return;
    }
    if (mentionResults.length > 0) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        mentionIndex = (mentionIndex + 1) % mentionResults.length;
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        mentionIndex = (mentionIndex - 1 + mentionResults.length) % mentionResults.length;
        return;
      }
      if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey)) {
        event.preventDefault();
        // Tab opens a folder; Enter picks it.
        void pickMention(mentionResults[Math.min(mentionIndex, mentionResults.length - 1)], event.key === 'Tab');
        return;
      }
      if (event.key === 'Escape') {
        event.stopPropagation();
        mentionDismissedAt = mention?.start ?? null;
        return;
      }
    }
    if (suggestions.length > 0) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        cmdIndex = (cmdIndex + 1) % suggestions.length;
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        cmdIndex = (cmdIndex - 1 + suggestions.length) % suggestions.length;
        return;
      }
      if (event.key === 'Tab' || (event.key === 'Enter' && !event.shiftKey)) {
        event.preventDefault();
        pickSuggestion(suggestions[Math.min(cmdIndex, suggestions.length - 1)].name);
        return;
      }
      if (event.key === 'Escape') {
        cmdDismissed = true;
        return;
      }
    }
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      submit();
      return;
    }
    if (event.key === 'Enter' && !event.shiftKey && enterSend) {
      event.preventDefault();
      submit();
    }
  }
</script>


<div class="composer" class:disabled class:streaming>
  {#if mention && (mentionResults.length > 0 || filesLoading)}
    <div class="suggest files" id={mentionListId} role="listbox" aria-label="Files">
      <div class="suggest-head">
        <span>{insideFolder ? `Inside ${insideFolder}` : 'Files in this folder'}</span>
        <span>{#if filesTruncated}<span title="Very large folder; showing the first 20,000 files">partial list · </span>{/if}Tab opens a folder</span>
      </div>
      {#if insideFolder && !filesLoading && !mentionResults.length}
        <div class="suggest-empty">Nothing to mention in {insideFolder}</div>
      {/if}
      {#if filesLoading && !mentionResults.length}
        <div class="suggest-empty">Loading files…</div>
      {/if}
      {#each mentionResults as path, i (path)}
        {@const parts = splitPath(path)}
        <button
          id={mentionOptionId(i)}
          type="button"
          role="option"
          aria-selected={i === mentionIndex}
          class="sug file"
          class:active={i === mentionIndex}
          onmousedown={(e) => { e.preventDefault(); void pickMention(path); }}
        >
          <span class="file-icon" aria-hidden="true">{#if path.endsWith('/')}<Folder size={13} strokeWidth={1.9} />{:else}<FileText size={13} strokeWidth={1.9} />{/if}</span>
          <span class="file-name">{parts.name}</span>
          <span class="file-dir">{insideFolder ? '' : parts.dir}</span>
          {#if path.endsWith('/')}
            <span class="open-folder" role="presentation" title="Open folder (Tab)" onmousedown={(e) => { e.preventDefault(); e.stopPropagation(); void pickMention(path, true); }}><ChevronRight size={13} strokeWidth={2} /></span>
          {/if}
        </button>
      {/each}
    </div>
  {:else if suggestions.length > 0}
    <div class="suggest" id={commandListId} role="listbox" aria-label="Commands">
      {#each suggestions as command, i (i + ':' + command.name)}
        <button
          id={optionId(i)}
          type="button"
          role="option"
          aria-selected={i === cmdIndex}
          class="sug"
          class:active={i === cmdIndex}
          onmousedown={(e) => {
            e.preventDefault();
            pickSuggestion(command.name);
          }}
        >
          <span class="sug-name">/{command.name}{#if command.args}<span class="sug-args"> {command.args}</span>{/if}</span>
          {#if command.description}<span class="sug-desc">{command.description}</span>{/if}
          <span class="sug-source" data-source={command.source}>{SOURCE_LABEL[command.source]}</span>
        </button>
      {/each}
    </div>
  {/if}

  <div class="box" class:asking={!!approval}>
    {#if approval && facts}
      <div class="approval" role="group" aria-label="Plan approval" transition:slide={{ duration: reduceMotion ? 0 : 420, easing: backOut }}>
        <div class="approval-head">
          <strong>Plan ready</strong>
          {#if facts.steps}<span class="fact">{facts.steps} {facts.steps === 1 ? 'step' : 'steps'}</span>{/if}
          {#if facts.files}<span class="fact">{facts.files} {facts.files === 1 ? 'file' : 'files'}</span>{/if}
          {#if facts.command}<span class="fact">runs <code>{facts.command}</code></span>{/if}
          {#if approval.onReview}<button type="button" class="link" onclick={approval.onReview}>Review plan</button>{/if}
        </div>
        {#if feedbackOpen}
          <!-- svelte-ignore a11y_autofocus -->
          <textarea class="feedback" bind:value={feedback} rows="2" maxlength="8000" aria-label="Plan feedback" placeholder="What should change?" autofocus
            onkeydown={(event) => { if (event.key === 'Enter' && (event.metaKey || !event.shiftKey)) { event.preventDefault(); sendFeedback(); } if (event.key === 'Escape') { event.stopPropagation(); feedbackOpen = false; } }}></textarea>
          <div class="approval-actions">
            <button type="button" class="a-btn ghost" disabled={answering} onclick={() => (feedbackOpen = false)}>Cancel</button>
            <button type="button" class="a-btn go" disabled={answering || !feedback.trim()} onclick={sendFeedback}>Send feedback</button>
          </div>
        {:else}
          <div class="approval-actions">
            <button type="button" class="a-btn ghost danger" disabled={answering} onclick={() => void answer(approval.onDecline)}>Decline</button>
            <button type="button" class="a-btn" disabled={answering} onclick={() => (feedbackOpen = true)}>Give feedback</button>
            <button type="button" class="a-btn go" disabled={answering} onclick={() => void answer(approval.onApprove)}>Approve &amp; run<kbd>⌘↵</kbd></button>
          </div>
        {/if}
      </div>
    {/if}
    <textarea
      bind:this={area}
      bind:value={text}
      onkeydown={onKeydown}
      role="combobox"
      aria-autocomplete="list"
      aria-haspopup="listbox"
      aria-expanded={suggestions.length > 0 || mentionResults.length > 0}
      aria-controls={mentionResults.length > 0 ? mentionListId : suggestions.length > 0 ? commandListId : undefined}
      aria-activedescendant={mentionResults.length > 0 ? mentionOptionId(mentionIndex) : suggestions.length > 0 ? optionId(cmdIndex) : undefined}
      oninput={() => { cmdDismissed = false; mentionDismissedAt = null; syncCaret(); }}
      onkeyup={(event) => { if (event.key.startsWith('Arrow') || event.key === 'Home' || event.key === 'End') syncCaret(); }}
      onclick={syncCaret}
      rows="1"
      class="input"
      placeholder={disabled
        ? 'Session disconnected — restart to continue'
        : approval
          ? 'Reply, or approve the plan above…'
        : streaming
          ? streamMode === 'steer'
            ? 'Steer the agent…'
            : 'Queue a follow-up…'
          : 'Ask the agent to build, fix, or explain…'}
      aria-label="Message"
      {disabled}
    ></textarea>

    <div class="toolbar">
      <div class="left">
        {#if onSetMode}
          <div class="agent-mode" role="radiogroup" aria-label="Agent mode">
            <button type="button" role="radio" aria-checked={agentMode === 'plan'} class:on={agentMode === 'plan'} title="Plan: read-only; asks before making changes" onclick={() => { if (agentMode !== 'plan') void onSetMode('plan'); }}><ClipboardList size={12} strokeWidth={2} />Plan</button>
            <button type="button" role="radio" aria-checked={agentMode === 'auto'} class:on={agentMode === 'auto'} title="Auto: full tools, no approval" onclick={() => { if (agentMode !== 'auto') void onSetMode('auto'); }}><Zap size={12} strokeWidth={2} />Auto</button>
          </div>
        {/if}
        {#if models.length > 0 && onSetModel}
          <div class="pill model picker-slot">
            <ModelPicker {models} current={currentModel} defaultKey={defaultModelKey} onSelect={onSetModel} {onMakeDefault} />
          </div>
        {:else if model}
          <span class="pill model static" title={`${model.name} · ${model.provider}`}><Cpu size={12} strokeWidth={2} />{model.name}</span>
        {/if}
        {#if levels.length > 0 && onSetEffort}
          <label class="pill effort" title="Reasoning effort">
            <Brain size={12} strokeWidth={2} />
            <select value={effort ?? ''} onchange={(e) => void onSetEffort(e.currentTarget.value)} aria-label="Select effort">
              {#if !effort}<option value="">Default effort</option>{/if}
              {#each levels as level (level)}<option value={level}>{capitalize(level)}</option>{/each}
            </select>
            <ChevronDown size={11} strokeWidth={2} />
          </label>
        {/if}
      </div>

      <div class="right">
        <ContextRing usage={contextUsage} />
        <button
          type="button"
          class="icon toggle"
          class:on={enterSend}
          aria-pressed={enterSend}
          aria-label="Send with Enter"
          title={enterSend ? 'Enter sends · Shift+Enter adds a newline' : '⌘Enter sends · click to send with Enter'}
          onclick={toggleEnterSend}
        >
          <CornerDownLeft size={13} strokeWidth={2} />
        </button>
        {#if streaming}
          <div class="mode">
            <button
              type="button"
              class="pill mode-btn"
              aria-haspopup="menu"
              aria-expanded={modeOpen}
              aria-label="Send mode while running"
              onclick={() => (modeOpen = !modeOpen)}
            >
              {streamMode === 'steer' ? 'Steer' : 'Queue'}
              <ChevronDown size={11} strokeWidth={2} />
            </button>
            {#if modeOpen}
              <div class="menu" role="menu">
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={streamMode === 'steer'}
                  class="mi"
                  onclick={() => {
                    streamMode = 'steer';
                    modeOpen = false;
                  }}><strong>Steer</strong><span>Interrupt with guidance</span></button
                >
                <button
                  type="button"
                  role="menuitemradio"
                  aria-checked={streamMode === 'follow_up'}
                  class="mi"
                  onclick={() => {
                    streamMode = 'follow_up';
                    modeOpen = false;
                  }}><strong>Queue</strong><span>Send when it finishes</span></button
                >
              </div>
            {/if}
          </div>
        {/if}
        {#if streaming}
          <button type="button" class="icon stop" onclick={() => void onAbort()} aria-label="Stop generation" title="Stop (Esc)">
            <Square size={11} strokeWidth={0} fill="currentColor" />
          </button>
        {/if}
        <button
          type="button"
          class="send"
          onclick={submit}
          disabled={!canSend}
          title={enterSend ? 'Send (Enter)' : 'Send (⌘Enter)'}
          aria-label={streaming ? (streamMode === 'steer' ? 'Send steer message' : 'Queue follow-up') : 'Send message'}
        >
          <ArrowUp size={15} strokeWidth={2.2} />
        </button>
      </div>
    </div>
  </div>
</div>

<style>
  .composer {
    position: relative;
    container: composer / inline-size;
  }
  .composer.disabled .box {
    opacity: 0.7;
  }
  .suggest {
    position: absolute;
    left: 0;
    right: 0;
    bottom: calc(100% + 6px);
    display: flex;
    flex-direction: column;
    padding: 4px;
    border-radius: var(--radius-lg);
    background: var(--elevated);
    box-shadow: var(--shadow);
    max-height: 240px;
    overflow-y: auto;
    z-index: 10;
    animation: ui-pop 0.12s var(--ease);
  }
  .sug {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 6px 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text);
    font-size: 12.5px;
    text-align: left;
  }
  .sug.active,
  .sug:hover {
    background: var(--accent-bg);
  }
  .sug-name {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--accent);
    flex: none;
  }
  .sug-desc {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .suggest-head {
    display: flex;
    justify-content: space-between;
    padding: 4px 10px 6px;
    color: var(--subtle);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .suggest-empty {
    padding: 6px 10px 8px;
    color: var(--muted);
    font-size: 12px;
  }
  .suggest.files {
    max-height: 320px;
    overflow-y: auto;
  }
  .sug.file {
    align-items: center;
    gap: 8px;
  }
  .file-icon {
    display: inline-flex;
    flex: none;
    color: var(--subtle);
  }
  .sug.file.active .file-icon {
    color: var(--accent);
  }
  .file-name {
    flex: none;
    max-width: 55%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
    font-size: 12.5px;
    font-weight: 500;
  }
  .open-folder {
    display: inline-flex;
    flex: none;
    margin-left: auto;
    padding: 2px;
    border-radius: 4px;
    color: var(--subtle);
  }
  .open-folder:hover,
  .sug.file.active .open-folder {
    background: var(--surface-2);
    color: var(--text);
  }
  .file-dir {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--subtle);
    font: 11.5px var(--mono);
  }
  .sug-args {
    color: var(--subtle);
  }
  .sug-source {
    flex: none;
    align-self: center;
    height: 17px;
    padding: 0 6px;
    border-radius: 5px;
    background: var(--surface-2);
    color: var(--subtle);
    font-size: 10.5px;
    font-weight: 600;
    line-height: 17px;
  }
  .sug-source[data-source='pidesk'] {
    background: var(--accent-bg);
    color: var(--accent);
  }
  .box {
    display: flex;
    flex-direction: column;
    gap: 14px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-xl);
    background: var(--elevated);
    padding: 16px 14px 12px;
    box-shadow: 0 4px 18px rgb(0 0 0 / 0.08), var(--shadow-sm);
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  .box.asking {
    border-color: color-mix(in srgb, var(--warn) 55%, var(--line-strong));
    box-shadow: 0 0 0 4px var(--warn-bg), 0 6px 24px rgb(0 0 0 / 0.12);
  }
  .approval {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin: -16px -14px 0;
    padding: 14px;
    border-bottom: 1px solid var(--line);
  }
  .approval-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    font-size: 13px;
  }
  .approval-head strong {
    margin-right: 2px;
    font-weight: 600;
  }
  .fact {
    height: 20px;
    padding: 0 7px;
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--muted);
    font-size: 11.5px;
    line-height: 20px;
    font-variant-numeric: tabular-nums;
  }
  .fact code {
    font-size: 11px;
  }
  .link {
    margin-left: auto;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: 12px;
    font-weight: 600;
  }
  .link:hover {
    text-decoration: underline;
  }
  .approval-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }
  .a-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 12px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text);
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
  }
  .a-btn:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .a-btn:disabled {
    opacity: 0.5;
  }
  .a-btn.ghost {
    border-color: transparent;
    background: none;
    color: var(--muted);
  }
  .a-btn.ghost.danger:hover:not(:disabled) {
    color: var(--bad);
    background: var(--bad-bg);
  }
  .a-btn.go {
    margin-left: auto;
    border-color: transparent;
    background: var(--accent-strong);
    color: var(--on-accent);
    box-shadow: var(--shadow-sm);
  }
  .a-btn.go:hover:not(:disabled) {
    filter: brightness(1.08);
    background: var(--accent-strong);
  }
  .a-btn kbd {
    border: 0;
    box-shadow: none;
    color: inherit;
    min-width: 0;
    height: auto;
    padding: 0 4px;
    border-radius: 4px;
    background: rgb(255 255 255 / 0.2);
    font-size: 10px;
    font-family: var(--font);
  }
  .feedback {
    width: 100%;
    resize: none;
    padding: 8px 10px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font: 13px/1.45 var(--font);
  }
  .box:focus-within {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--line-strong));
  }
  .input {
    width: 100%;
    min-height: 26px;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 14.5px;
    line-height: 1.5;
    resize: none;
    outline: none;
    max-height: 180px;
    font-family: inherit;
    padding: 0 4px 0 0;
  }
  .input:focus-visible {
    box-shadow: none;
  }
  .input::placeholder {
    color: var(--subtle);
  }
  .toolbar {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 10px 12px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
  }
  .left,
  .right {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .left {
    flex: 1 1 360px;
    flex-wrap: wrap;
  }
  .right {
    flex: none;
    margin-left: auto;
  }
  .pill {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 10px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--muted);
    font-size: 13px;
    font-weight: 500;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .agent-mode {
    display: inline-flex;
    flex: none;
    padding: 2px;
    border-radius: 999px;
    background: var(--surface-2);
  }
  .agent-mode button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
  }
  .agent-mode button:hover:not(.on) {
    color: var(--text);
  }
  .agent-mode button.on {
    background: var(--bg);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
  .agent-mode button:focus-visible {
    outline: none;
    box-shadow: var(--focus-ring);
  }
  .pill.model {
    flex-shrink: 1;
    min-width: 0;
    max-width: 100%;
    overflow: hidden;
  }
  .picker-slot {
    padding: 0;
  }
  .picker-slot:hover,
  .picker-slot:focus-within {
    background: transparent;
  }
  .pill:hover:not(.static),
  .pill:focus-within {
    background: var(--surface-2);
    color: var(--text);
  }
  .pill select {
    appearance: none;
    border: 0;
    background: transparent;
    color: inherit;
    font-size: inherit;
    font-weight: inherit;
    padding: 0;
    max-width: 160px;
    text-overflow: ellipsis;
    cursor: pointer;
    field-sizing: content;
  }
  .pill select:focus-visible {
    box-shadow: none;
  }
  .pill :global(svg) {
    flex: none;
    pointer-events: none;
  }
  .pill.static {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .mode {
    position: relative;
  }
  .mode-btn {
    color: var(--accent);
    background: var(--accent-bg);
  }
  .menu {
    position: absolute;
    bottom: calc(100% + 6px);
    right: 0;
    display: flex;
    flex-direction: column;
    min-width: 220px;
    padding: 4px;
    border-radius: var(--radius-lg);
    background: var(--elevated);
    box-shadow: var(--shadow);
    z-index: 10;
    animation: ui-pop 0.12s var(--ease);
  }
  .mi {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 6px 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text);
    font-size: 12.5px;
    text-align: left;
  }
  .mi strong {
    font-weight: 600;
  }
  .mi span {
    color: var(--muted);
    font-size: 12px;
  }
  .mi:hover,
  .mi[aria-checked='true'] {
    background: var(--accent-bg);
  }
  .icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--subtle);
  }
  .icon:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .toggle.on {
    color: var(--accent);
  }
  .stop {
    color: var(--text);
    background: var(--surface-2);
  }
  .stop:hover {
    color: var(--bad);
    background: var(--bad-bg);
  }
  .send {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    margin-left: 2px;
    border: 0;
    border-radius: 999px;
    background: var(--accent-strong);
    color: var(--on-accent);
    transition: filter 0.12s, background 0.12s;
  }
  .send:hover:not(:disabled) {
    filter: brightness(1.1);
  }
  @container composer (max-width: 560px) {
    .left {
      flex-basis: 220px;
    }
    .toolbar:has(.pill.model) {
      display: grid;
      grid-template-columns: auto minmax(0, 1fr);
      gap: 8px;
    }
    .toolbar:has(.pill.model) .left {
      display: contents;
    }
    .agent-mode {
      grid-area: 1 / 1;
      justify-self: start;
    }
    .pill.model {
      grid-area: 1 / 2;
      justify-self: end;
    }
    .effort {
      grid-area: 2 / 1;
      justify-self: start;
    }
    .pill select {
      max-width: 112px;
    }
    .right {
      grid-area: 2 / 2;
      justify-self: end;
      justify-content: flex-end;
      flex-wrap: wrap;
      gap: 6px;
    }
  }
  .send:disabled {
    background: var(--surface-3);
    color: var(--subtle);
    opacity: 1;
  }
</style>
