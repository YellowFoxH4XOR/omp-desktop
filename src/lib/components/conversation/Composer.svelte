<script lang="ts">
  import { ArrowUp, Brain, ChevronDown, CornerDownLeft, Cpu, Square } from '@lucide/svelte';
  import type { ContextUsage, ModelInfo, ThreadStatus } from '../../types';
  import ModelPicker from './ModelPicker.svelte';
  import { modelKey } from './model-utils';

  type SendMode = 'prompt' | 'steer' | 'follow_up';

  interface Props {
    threadId: string;
    status: ThreadStatus;
    commands?: Array<{ name: string; description?: string }>;
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
  }: Props = $props();

  // Session state carries a slim model; the catalogue entry has context and traits.
  const currentModel = $derived(model ? (models.find((entry) => modelKey(entry) === modelKey(model)) ?? model) : null);
  const contextPercent = $derived(
    contextUsage?.percent != null ? Math.max(0, Math.min(100, Math.round(contextUsage.percent))) : null,
  );
  const RING = 2 * Math.PI * 6;
  function capitalize(value: string): string {
    return value ? value[0].toUpperCase() + value.slice(1) : value;
  }

  let text = $state('');
  let submitting = $state(false);
  let area = $state<HTMLTextAreaElement>();
  let enterSend = $state(false);
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

  const suggestions = $derived.by(() => {
    if (cmdDismissed || !text.startsWith('/') || text.includes(' ') || text.includes('\n')) {
      return [] as Array<{ name: string; description?: string }>;
    }
    const needle = text.slice(1).toLowerCase();
    return commands
      .filter((command) => command.name.toLowerCase().startsWith(needle))
      .slice(0, 8);
  });

  // Focus the composer on mount and whenever a different thread is shown.
  $effect(() => {
    threadId;
    submitGeneration += 1;
    submitting = false;
    text = '';
    cmdDismissed = false;
    area?.focus();
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

  function onKeydown(event: KeyboardEvent) {
    // IME commits send keydown with isComposing/keyCode 229: never submit or
    // pick a suggestion mid-composition; the post-commit Enter works normally.
    if (event.isComposing || event.keyCode === 229) return;
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
  {#if suggestions.length > 0}
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
          <span class="sug-name">/{command.name}</span>
          {#if command.description}<span class="sug-desc">{command.description}</span>{/if}
        </button>
      {/each}
    </div>
  {/if}

  <div class="box">
    <textarea
      bind:this={area}
      bind:value={text}
      onkeydown={onKeydown}
      role="combobox"
      aria-autocomplete="list"
      aria-haspopup="listbox"
      aria-expanded={suggestions.length > 0}
      aria-controls={suggestions.length > 0 ? commandListId : undefined}
      aria-activedescendant={suggestions.length > 0 ? optionId(cmdIndex) : undefined}
      oninput={() => (cmdDismissed = false)}
      rows="1"
      class="input"
      placeholder={disabled
        ? 'Session disconnected — restart to continue'
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
        {#if models.length > 0 && onSetModel}
          <div class="pill model picker-slot">
            <ModelPicker {models} current={currentModel} defaultKey={defaultModelKey} onSelect={onSetModel} {onMakeDefault} />
          </div>
        {:else if model}
          <span class="pill model static" title={`${model.name} · ${model.provider}`}><Cpu size={12} strokeWidth={2} />{model.name}</span>
        {/if}
        {#if levels.length > 0 && onSetEffort}
          <label class="pill" title="Reasoning effort">
            <Brain size={12} strokeWidth={2} />
            <select value={effort ?? ''} onchange={(e) => void onSetEffort(e.currentTarget.value)} aria-label="Select effort">
              {#if !effort}<option value="">Default effort</option>{/if}
              {#each levels as level (level)}<option value={level}>{capitalize(level)}</option>{/each}
            </select>
            <ChevronDown size={11} strokeWidth={2} />
          </label>
        {/if}
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
      </div>

      <div class="right">
        {#if contextPercent !== null}
          <span
            class="context"
            class:high={contextPercent >= 80}
            title={`Context ${contextUsage?.tokens?.toLocaleString() ?? '?'} / ${contextUsage?.contextWindow.toLocaleString()} tokens`}
          >
            <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
              <circle cx="8" cy="8" r="6" class="ring-track" />
              <circle cx="8" cy="8" r="6" class="ring-fill" stroke-dasharray={`${(RING * contextPercent) / 100} ${RING}`} />
            </svg>
            <span class="context-pct">{contextPercent}%</span>
          </span>
        {/if}
        <button
          type="button"
          class="icon toggle"
          class:on={enterSend}
          aria-pressed={enterSend}
          aria-label="Send with Enter"
          title={enterSend ? 'Enter sends · Shift+Enter adds a newline' : '⌘Enter sends · click to send with Enter'}
          onclick={() => (enterSend = !enterSend)}
        >
          <CornerDownLeft size={13} strokeWidth={2} />
        </button>
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
    color: var(--muted);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-xl);
    background: var(--elevated);
    padding: 12px 10px 8px 14px;
    box-shadow: 0 6px 24px rgb(0 0 0 / 0.12), var(--shadow-sm);
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  .box:focus-within {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--line-strong));
  }
  .input {
    width: 100%;
    min-height: 22px;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 14px;
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
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-left: -6px;
  }
  .left,
  .right {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }
  .left {
    flex: 1;
    overflow: hidden;
  }
  .right {
    flex: none;
  }
  .pill {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 8px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
    flex-shrink: 0;
    white-space: nowrap;
  }
  .pill.model {
    flex-shrink: 1;
    min-width: 0;
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
    left: 0;
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
    font-size: 11.5px;
  }
  .mi:hover,
  .mi[aria-checked='true'] {
    background: var(--accent-bg);
  }
  .context {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0 6px;
    color: var(--subtle);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .context svg {
    transform: rotate(-90deg);
  }
  .ring-track {
    fill: none;
    stroke: var(--surface-3);
    stroke-width: 2.2;
  }
  .ring-fill {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2.2;
    stroke-linecap: round;
  }
  .context.high .ring-fill {
    stroke: var(--warn);
  }
  .icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
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
    width: 30px;
    height: 30px;
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
    .pill > :global(svg:first-child),
    .context-pct {
      display: none;
    }
    .pill select {
      max-width: 96px;
    }
  }
  @container composer (max-width: 420px) {
    .toggle,
    .pill.model {
      display: none;
    }
    .pill select {
      max-width: 72px;
    }
  }
  .send:disabled {
    background: var(--surface-3);
    color: var(--subtle);
    opacity: 1;
  }
</style>
