<script lang="ts">
  import { ArrowUp, ChevronDown, Square } from '@lucide/svelte';
  import type { ThreadStatus } from '../../types';

  type SendMode = 'prompt' | 'steer' | 'follow_up';

  interface Props {
    threadId: string;
    status: ThreadStatus;
    commands?: Array<{ name: string; description?: string }>;
    onSend: (message: string, mode: SendMode) => void | Promise<void>;
    onAbort: () => void | Promise<void>;
  }

  let { threadId, status, commands = [], onSend, onAbort }: Props = $props();

  let text = $state('');
  let submitting = $state(false);
  let area = $state<HTMLTextAreaElement>();
  let enterSend = $state(false);
  let streamMode = $state<'steer' | 'follow_up'>('steer');
  let modeOpen = $state(false);
  let cmdIndex = $state(0);
  let cmdDismissed = $state(false);

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

  async function submit() {
    const message = text.trim();
    if (!message || disabled || submitting) return;
    submitting = true;
    try {
      await onSend(message, streaming ? streamMode : 'prompt');
      // If the user started a new draft during the request, leave it alone.
      if (text.trim() === message) text = '';
    } catch {
      area?.focus();
    } finally {
      submitting = false;
    }
  }

  function pickSuggestion(name: string) {
    text = `/${name} `;
    cmdDismissed = false;
    area?.focus();
  }

  function onKeydown(event: KeyboardEvent) {
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

<div class="composer" class:disabled>
  {#if suggestions.length > 0}
    <div class="suggest" role="listbox" aria-label="Commands">
      {#each suggestions as command, i (command.name)}
        <button
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
      oninput={() => (cmdDismissed = false)}
      rows="1"
      class="input"
      placeholder={streaming
        ? streamMode === 'steer'
          ? 'Steer the agent…'
          : 'Queue a follow-up…'
        : 'Message the agent…'}
      aria-label="Message"
      {disabled}
    ></textarea>

    <div class="controls">
      {#if streaming}
        <div class="mode">
          <button
            type="button"
            class="mode-btn"
            aria-haspopup="menu"
            aria-expanded={modeOpen}
            aria-label="Send mode while running"
            onclick={() => (modeOpen = !modeOpen)}
          >
            {streamMode === 'steer' ? 'Steer' : 'Queue'}
            <ChevronDown size={11} />
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
                }}>Steer — interrupt with guidance</button
              >
              <button
                type="button"
                role="menuitemradio"
                aria-checked={streamMode === 'follow_up'}
                class="mi"
                onclick={() => {
                  streamMode = 'follow_up';
                  modeOpen = false;
                }}>Queue — send when it finishes</button
              >
            </div>
          {/if}
        </div>
      {/if}

      <button
        type="button"
        class="toggle"
        class:on={enterSend}
        aria-pressed={enterSend}
        aria-label="Send with Enter"
        title="Send with Enter"
        onclick={() => (enterSend = !enterSend)}
      >
        ⏎
      </button>

      {#if streaming}
        <button type="button" class="send stop" onclick={() => void onAbort()} aria-label="Stop generation">
          <Square size={12} />
        </button>
      {/if}
      <button
        type="button"
        class="send"
        onclick={submit}
        disabled={!canSend}
        aria-label={streaming ? (streamMode === 'steer' ? 'Send steer message' : 'Queue follow-up') : 'Send message'}
      >
        <ArrowUp size={14} />
      </button>
    </div>
  </div>

  <div class="hint">
    {enterSend ? 'Enter to send · Shift+Enter for newline' : '⌘+Enter to send · Shift+Enter for newline'}
  </div>
</div>

<style>
  .composer {
    padding: 8px 12px 10px;
    border-top: 1px solid var(--line);
    background: var(--bg);
  }
  .composer.disabled {
    opacity: 0.7;
  }
  .suggest {
    display: flex;
    flex-direction: column;
    margin-bottom: 6px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
    max-height: 200px;
    overflow-y: auto;
  }
  .sug {
    display: flex;
    gap: 8px;
    align-items: baseline;
    padding: 5px 9px;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 12px;
    text-align: left;
  }
  .sug.active,
  .sug:hover {
    background: var(--surface-2);
  }
  .sug-name {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--accent);
    flex: none;
  }
  .sug-desc {
    color: var(--muted);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .box {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    padding: 7px 8px 7px 10px;
  }
  .box:focus-within {
    border-color: var(--accent);
  }
  .input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 13px;
    line-height: 1.45;
    resize: none;
    outline: none;
    max-height: 180px;
    font-family: inherit;
  }
  .input::placeholder {
    color: var(--subtle);
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .mode {
    position: relative;
  }
  .mode-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--muted);
    font-size: 11px;
  }
  .mode-btn:hover {
    color: var(--text);
  }
  .menu {
    position: absolute;
    bottom: calc(100% + 4px);
    right: 0;
    display: flex;
    flex-direction: column;
    min-width: 220px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: var(--shadow);
    overflow: hidden;
    z-index: 10;
  }
  .mi {
    padding: 6px 10px;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 12px;
    text-align: left;
  }
  .mi:hover {
    background: var(--surface-2);
  }
  .toggle {
    padding: 4px 7px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: transparent;
    color: var(--muted);
    font-size: 11px;
    line-height: 1;
  }
  .toggle.on {
    color: var(--accent);
    border-color: var(--accent);
  }
  .send {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: 6px;
    background: var(--accent-bg);
    color: var(--text);
  }
  .send:hover:not(:disabled) {
    filter: brightness(1.15);
  }
  .send.stop {
    background: var(--surface-2);
    color: var(--bad);
    border: 1px solid var(--line);
  }
  .hint {
    margin-top: 5px;
    color: var(--subtle);
    font-size: 10.5px;
  }
</style>
