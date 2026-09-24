<script lang="ts">
  import { onDestroy } from 'svelte';
  import { VList, type VListHandle } from 'virtua/svelte';
  import { Brain, ChevronDown, ChevronRight, Info, Sparkles, TriangleAlert } from '@lucide/svelte';
  import type { AgentInfo, ConversationItem } from '../../types';
  import CommandCard from '../tools/CommandCard.svelte';
  import ToolCard from '../tools/ToolCard.svelte';
  import type { ToolItem } from '../tools/tool-utils';
  import Markdown from './Markdown.svelte';

  interface Props {
    items: ConversationItem[];
    agents?: AgentInfo[];
    status?: string;
    onShowChanges?: (path?: string) => void;
    onShowAgents?: () => void;
  }

  let { items, agents = [], status = 'idle', onShowChanges, onShowAgents }: Props = $props();

  let list = $state<VListHandle>();
  let wrap = $state<HTMLDivElement>();
  let stickToBottom = $state(true);
  let virtualCount = $state<number>();
  const renderedCount = $derived(virtualCount ?? items.length);
  const renderedItems = $derived(
    renderedCount >= items.length ? items : items.slice(0, renderedCount),
  );
  let pinFrame: number | undefined;
  let firstItemId: string | undefined;
  let completionAnnouncement = $state('');

  function scheduleBottomPin() {
    if (pinFrame !== undefined) return;
    pinFrame = requestAnimationFrame(() => {
      pinFrame = undefined;
      const count = renderedItems.length;
      if (stickToBottom && count > 0) list?.scrollToIndex(count - 1, { align: 'end' });
    });
  }

  onDestroy(() => {
    if (pinFrame !== undefined) cancelAnimationFrame(pinFrame);
  });

  // A new array after a live append is still the same transcript. Reset
  // pinning only when navigation changes its first stable item.
  $effect(() => {
    const first = items[0]?.id;
    if (firstItemId !== undefined && first !== firstItemId) stickToBottom = true;
    firstItemId = first;
  });

  function onScroll(offset: number) {
    if (!list) return;
    const distance = list.getScrollSize() - list.getViewportSize() - offset;
    stickToBottom = distance < 80;
  }
  // While the user reads older messages, leave newly appended rows out of the
  // list data. Updating that data preserves VList's measured scroll anchor;
  // rendering the deferred rows later is an ordinary keyed append.
  $effect(() => {
    if (stickToBottom && virtualCount !== items.length) virtualCount = items.length;
  });

  // Announce only when the run leaves streaming, not when one text block
  // settles before a tool call or the next content block.
  let announcedRun = false;
  $effect(() => {
    const running = status === 'active' || status === 'waiting';
    if (running) {
      announcedRun = true;
      completionAnnouncement = '';
      return;
    }
    if (announcedRun && (status === 'completed' || status === 'idle' || status === 'failed')) {
      completionAnnouncement = status === 'failed' ? 'Assistant response failed.' : 'Assistant response complete.';
      announcedRun = false;
    }
  });


  // Only the trailing row grows during streaming. Avoid walking the full
  // transcript for every token in a long conversation.
  const contentSig = $derived.by(() => {
    const last = items.at(-1);
    if (!last) return 'empty';
    const size = last.kind === 'tool'
      ? `${last.status}:${Boolean(last.result)}:${Boolean(last.partial)}`
      : `${last.text.length}:${'streaming' in last && Boolean(last.streaming)}`;
    return `${items.length}:${last.id}:${size}`;
  });

  $effect(() => {
    contentSig;
    if (stickToBottom) scheduleBottomPin();
  });
  // CodeMirror/tool cards and virtual rows change height after mount. Follow
  // those measurements only while the user is at the bottom, without polling.
  $effect(() => {
    const spacer = wrap?.firstElementChild?.firstElementChild;
    if (!(spacer instanceof HTMLElement)) return;
    const observer = new ResizeObserver(() => {
      if (stickToBottom) scheduleBottomPin();
    });
    observer.observe(spacer);
    return () => observer.disconnect();
  });


  function scrollToBottom() {
    stickToBottom = true;
    virtualCount = items.length;
    scheduleBottomPin();
  }

  type CustomItem = Extract<ConversationItem, { kind: 'custom' | 'notice' | 'advisor' }>;

  /** bashExecution custom messages render through the command card. */
  function asCommandItem(item: CustomItem): ToolItem {
    const details =
      item.details && typeof item.details === 'object' && !Array.isArray(item.details)
        ? (item.details as Record<string, unknown>)
        : {};
    const code = typeof details['exitCode'] === 'number' ? details['exitCode'] : undefined;
    return {
      id: item.id,
      kind: 'tool',
      toolCallId: item.id,
      toolName: 'bash',
      args: { command: typeof details['command'] === 'string' ? details['command'] : item.text },
      status: code !== undefined && code !== 0 ? 'failed' : 'completed',
      result: {
        details,
        isError: code !== undefined && code !== 0,
      },
      timestamp: item.timestamp,
    };
  }
</script>

<div class="transcript">
  {#if items.length === 0}
    <div class="empty">
      <p>No messages yet.</p>
      <p class="muted">Send a message to start the session.</p>
    </div>
  {:else}
    <div class="list-wrap" bind:this={wrap}>
      <VList
        bind:this={list}
        data={renderedItems}
        getKey={(item) => item.id}
        onscroll={onScroll}
        bufferSize={400}
        ssrCount={12}
      >
        {#snippet children(item)}
          <div class="row">
            {#if item.kind === 'user'}
              <div class="user">
                <div class="user-text">{item.text}</div>
              </div>
            {:else if item.kind === 'text'}
              <div class="assistant">
                <Markdown text={item.text} streaming={item.streaming ?? false} />
                {#if item.streaming}<span class="caret" aria-hidden="true"></span>{/if}
              </div>
            {:else if item.kind === 'thinking'}
              <details class="thinking" open={item.streaming ?? false}>
                <summary>
                  <Brain size={12} />
                  <span>{item.streaming ? 'Thinking…' : 'Thought'}</span>
                </summary>
                <div class="think-body">{item.text}</div>
              </details>
            {:else if item.kind === 'tool'}
              <ToolCard {item} {agents} {onShowChanges} {onShowAgents} />
            {:else if item.kind === 'advisor'}
              <div class="advisor">
                <div class="advisor-head">
                  <Sparkles size={12} />
                  <span>Advisor</span>
                </div>
                <Markdown text={item.text} />
              </div>
            {:else if item.kind === 'notice'}
              <div class="notice" class:warn={item.level === 'warn'} class:error={item.level === 'error'}>
                {#if item.level === 'error' || item.level === 'warn'}
                  <TriangleAlert size={12} />
                {:else}
                  <Info size={12} />
                {/if}
                <span>{item.text}</span>
              </div>
            {:else if item.kind === 'custom'}
              {#if item.customType === 'bashExecution'}
                <CommandCard item={asCommandItem(item)} />
              {:else}
                <details class="custom">
                  <summary>
                    <ChevronRight size={11} class="c-closed" />
                    <ChevronDown size={11} class="c-open" />
                    <span class="custom-type">{item.customType ?? 'event'}</span>
                    <span class="custom-text">{item.text.slice(0, 100)}</span>
                  </summary>
                  <div class="custom-body">{item.text}</div>
                </details>
              {/if}
            {/if}
          </div>
        {/snippet}
      </VList>
    </div>

    {#if !stickToBottom}
      <button type="button" class="to-bottom" onclick={scrollToBottom} aria-label="Scroll to latest">
        <ChevronDown size={13} /> Latest
      </button>
    {/if}
  {/if}
  <div class="sr-only" role="status" aria-live="polite" aria-atomic="true">
    {completionAnnouncement}
  </div>
</div>

<style>
  .transcript {
    position: relative;
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
  .list-wrap {
    flex: 1;
    min-height: 0;
  }
  .row {
    padding: 2px 16px;
    user-select: text;
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    color: var(--text);
    font-size: 13px;
  }
  .empty .muted {
    color: var(--muted);
    font-size: 12px;
  }
  .user {
    display: flex;
    justify-content: flex-end;
    margin: 6px 0;
  }
  .user-text {
    max-width: 78%;
    padding: 6px 10px;
    border-radius: 10px;
    background: var(--surface-2);
    font-size: 13px;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .assistant {
    margin: 4px 0 8px;
    max-width: 100%;
  }
  .caret {
    display: inline-block;
    width: 7px;
    height: 14px;
    margin-left: 2px;
    vertical-align: text-bottom;
    background: var(--accent);
    animation: blink 1s steps(2) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .thinking {
    margin: 3px 0;
    border-left: 2px solid var(--line);
    padding-left: 10px;
    color: var(--muted);
  }
  .thinking summary {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    font-size: 11.5px;
    list-style: none;
    padding: 2px 0;
  }
  .thinking summary::-webkit-details-marker {
    display: none;
  }
  .think-body {
    font-size: 12px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding: 2px 0 6px;
    font-style: italic;
  }
  .advisor {
    margin: 6px 0;
    border: 1px solid var(--accent-bg);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius);
    background: var(--surface);
    padding: 7px 10px;
  }
  .advisor-head {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 4px;
  }
  .notice {
    display: flex;
    align-items: baseline;
    gap: 7px;
    margin: 3px 0;
    padding: 4px 8px;
    border-radius: 6px;
    color: var(--muted);
    font-size: 11.5px;
  }
  .notice.warn {
    color: var(--warn);
  }
  .notice.error {
    color: var(--bad);
  }
  .custom {
    margin: 3px 0;
    font-size: 12px;
    color: var(--muted);
  }
  .custom summary {
    display: flex;
    align-items: center;
    gap: 5px;
    cursor: pointer;
    list-style: none;
    padding: 2px 0;
    min-width: 0;
  }
  .custom summary::-webkit-details-marker {
    display: none;
  }
  .custom summary :global(.c-open) {
    display: none;
  }
  .custom[open] summary :global(.c-open) {
    display: inline-flex;
  }
  .custom[open] summary :global(.c-closed) {
    display: none;
  }
  .custom-type {
    flex: none;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--subtle);
  }
  .custom-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .custom-body {
    padding: 4px 0 4px 16px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--text);
    font-size: 12px;
  }
  .to-bottom {
    position: absolute;
    bottom: 10px;
    right: 16px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    border: 1px solid var(--line);
    border-radius: 14px;
    background: var(--surface);
    color: var(--text);
    font-size: 11.5px;
    box-shadow: var(--shadow);
  }
  .to-bottom:hover {
    background: var(--surface-2);
  }
</style>
