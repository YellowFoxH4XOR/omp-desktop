<script lang="ts">
  import { onDestroy } from 'svelte';
  import { VList, type VListHandle } from 'virtua/svelte';
  import { ArrowDown, Brain, Info, Puzzle, Sparkles, TriangleAlert } from '@lucide/svelte';
  import type { ConversationItem } from '../../types';
  import CommandCard from '../tools/CommandCard.svelte';
  import ToolCard from '../tools/ToolCard.svelte';
  import ToolShell from '../tools/ToolShell.svelte';
  import { preview } from '../tools/tool-utils';
  import type { ToolItem } from '../tools/tool-utils';
  import Markdown from './Markdown.svelte';

  interface Props {
    items: ConversationItem[];
    status?: string;
    onShowChanges?: (path?: string) => void;
  }

  let { items, status = 'idle', onShowChanges }: Props = $props();

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

  /** Tool calls, thinking, and harness events render as one connected timeline. */
  function isStep(item: ConversationItem | undefined): boolean {
    return !!item && (item.kind === 'tool' || item.kind === 'thinking' || item.kind === 'custom');
  }

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
      <div class="empty-mark"><Sparkles size={18} strokeWidth={1.6} /></div>
      <p>No messages yet</p>
      <p class="muted">Describe what you want to build, fix, or understand.</p>
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
        {#snippet children(item, index)}
          {@const step = isStep(item)}
          <div
            class="row kind-{item.kind}"
            class:step
            class:step-first={step && !isStep(renderedItems[index - 1])}
            class:step-last={step && !isStep(renderedItems[index + 1])}
            class:first={index === 0}
          >
            <div class="row-inner">
              {#if item.kind === 'user'}
                <div class="user">
                  <div class="user-text">{item.text}</div>
                </div>
              {:else if item.kind === 'text'}
                <div class="assistant" class:streaming={item.streaming}>
                  <Markdown text={item.text} streaming={item.streaming ?? false} />
                </div>
              {:else if item.kind === 'thinking'}
                <ToolShell status={item.streaming ? 'running' : 'completed'} icon={Brain} initiallyOpen={item.streaming ?? false} quiet>
                  {#snippet summary()}
                    <span class="think-label">{item.streaming ? 'Thinking' : 'Thought'}</span>
                    {#if !item.streaming && item.text}<span class="think-preview">{preview(item.text, 110)}</span>{/if}
                  {/snippet}
                  {#snippet detail()}
                    <div class="think-body">{item.text}</div>
                  {/snippet}
                </ToolShell>
              {:else if item.kind === 'tool'}
                <ToolCard {item} {onShowChanges} />
              {:else if item.kind === 'advisor'}
                <div class="advisor">
                  <div class="advisor-head">
                    <Sparkles size={12} strokeWidth={2} />
                    <span>Advisor</span>
                  </div>
                  <Markdown text={item.text} />
                </div>
              {:else if item.kind === 'notice'}
                <div class="notice" class:warn={item.level === 'warn'} class:error={item.level === 'error'}>
                  {#if item.level === 'error' || item.level === 'warn'}
                    <TriangleAlert size={12} strokeWidth={2} />
                  {:else}
                    <Info size={12} strokeWidth={2} />
                  {/if}
                  <span>{item.text}</span>
                </div>
              {:else if item.kind === 'custom'}
                {#if item.customType === 'bashExecution'}
                  <CommandCard item={asCommandItem(item)} />
                {:else}
                  <ToolShell status="completed" icon={Puzzle} quiet>
                    {#snippet summary()}
                      <span class="custom-type">{item.customType ?? 'event'}</span>
                      <span class="think-preview">{preview(item.text, 100)}</span>
                    {/snippet}
                    {#snippet detail()}
                      <div class="custom-body">{item.text}</div>
                    {/snippet}
                  </ToolShell>
                {/if}
              {/if}
            </div>
          </div>
        {/snippet}
      </VList>
    </div>

    {#if !stickToBottom}
      <button type="button" class="to-bottom" onclick={scrollToBottom} aria-label="Scroll to latest">
        <ArrowDown size={13} strokeWidth={2} /> Latest
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
    padding: 3px 0;
    user-select: text;
  }
  .row.first {
    padding-top: 24px;
  }
  .row-inner {
    position: relative;
    max-width: calc(var(--content-width) + 48px);
    margin: 0 auto;
    padding: 0 24px;
  }
  .row.kind-user {
    padding: 14px 0 10px;
  }
  .row.kind-user.first {
    padding-top: 24px;
  }
  .row.step {
    padding: 0;
  }
  .row.step-first {
    padding-top: 6px;
  }
  .row.step-last {
    padding-bottom: 8px;
  }
  /* Timeline rail through the step nodes (node centre: 24px gutter + 4px + 10px). */
  .row.step .row-inner::before {
    content: '';
    position: absolute;
    left: 37.5px;
    top: 0;
    bottom: 0;
    width: 1px;
    background: color-mix(in srgb, var(--subtle) 40%, transparent);
  }
  .row.step-first .row-inner::before {
    top: 14px;
  }
  .row.step-last .row-inner::before {
    bottom: auto;
    height: 14px;
  }
  .row.step-first.step-last .row-inner::before {
    display: none;
  }
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    color: var(--text);
    font-size: 13.5px;
  }
  .empty p {
    margin: 0;
  }
  .empty-mark {
    width: 40px;
    height: 40px;
    margin-bottom: 12px;
    border-radius: 12px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--accent);
    background: var(--accent-bg);
  }
  .empty .muted {
    color: var(--muted);
    font-size: 12.5px;
  }
  .user {
    display: flex;
    justify-content: flex-end;
  }
  .user-text {
    max-width: 82%;
    padding: 9px 14px;
    border-radius: 18px 18px 6px 18px;
    background: var(--surface-2);
    font-size: 14px;
    line-height: 1.55;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .assistant {
    padding: 4px 0;
    max-width: 100%;
    font-size: 14px;
  }
  /* Inline caret after the last streamed block (or on its own line when empty). */
  .assistant.streaming :global(.md > :last-child)::after,
  .assistant.streaming :global(.md:empty)::after {
    content: '';
    display: inline-block;
    width: 7px;
    height: 15px;
    margin-left: 3px;
    border-radius: 2px;
    vertical-align: -2px;
    background: var(--accent);
    animation: blink 1.1s steps(2) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .think-label {
    color: var(--muted);
    font-weight: 500;
  }
  .think-preview {
    margin-left: 6px;
    color: var(--subtle);
  }
  .think-body {
    color: var(--muted);
    font-size: 12.5px;
    line-height: 1.6;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .advisor {
    margin: 6px 0;
    border-radius: var(--radius-lg);
    background: var(--accent-bg);
    padding: 10px 14px;
  }
  .advisor-head {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--accent);
    font-size: 11.5px;
    font-weight: 600;
    margin-bottom: 4px;
  }
  .notice {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin: 6px 0;
    color: var(--subtle);
    font-size: 12px;
    text-align: center;
  }
  .notice :global(svg) {
    flex: none;
  }
  .notice.warn {
    color: var(--warn);
  }
  .notice.error {
    color: var(--bad);
  }
  .custom-type {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--muted);
  }
  .custom-body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    color: var(--text);
    font-size: 12.5px;
  }
  .to-bottom {
    position: absolute;
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 28px;
    padding: 0 12px;
    border: 0;
    border-radius: 999px;
    background: var(--elevated);
    color: var(--text);
    font-size: 12px;
    font-weight: 500;
    box-shadow: var(--shadow);
    animation: ui-rise 0.15s var(--ease);
  }
  .to-bottom:hover {
    color: var(--accent);
  }
</style>
