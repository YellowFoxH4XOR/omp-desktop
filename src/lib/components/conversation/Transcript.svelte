<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { VList, type VListHandle } from 'virtua/svelte';
  import { ArrowDown, Brain, ChevronRight, Code2, FileText, Globe, Info, Pencil, Plug, Puzzle, Search, Sparkles, SquareTerminal, TriangleAlert } from '@lucide/svelte';
  import { mcpCall } from '../tools/mcp-tools.svelte';
  import type { ConversationItem } from '../../types';
  import CommandCard from '../tools/CommandCard.svelte';
  import ToolCard from '../tools/ToolCard.svelte';
  import ToolShell from '../tools/ToolShell.svelte';
  import { preview } from '../tools/tool-utils';
  import type { ToolItem } from '../tools/tool-utils';
  import Markdown from './Markdown.svelte';
  import WorkingLine from './WorkingLine.svelte';

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
      const count = rows.length;
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


  // Rows that arrive while the thread is open slide in; history does not.
  // Only the first sighting counts, so rows scrolled back into view stay put.
  const firstSeen = new Map<string, number>();
  let historyIds = new Set<string>();
  $effect(() => {
    const first = items[0]?.id;
    untrack(() => {
      if (first === undefined || historyIds.has(first)) return;
      historyIds = new Set(items.map(item => item.id));
      firstSeen.clear();
    });
  });
  function isFresh(id: string): boolean {
    if (historyIds.has(id)) return false;
    const now = performance.now();
    let seen = firstSeen.get(id);
    if (seen === undefined) {
      if (firstSeen.size > 2000) firstSeen.clear();
      firstSeen.set(id, (seen = now));
    }
    return now - seen < 500;
  }

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

  // Consecutive steps render as one row. A finished run of three or more
  // folds into a summary line; the live run stays open as it happens.
  type Row =
    | { kind: 'item'; id: string; item: ConversationItem }
    | { kind: 'steps'; id: string; items: ConversationItem[]; end?: number };
  const rows = $derived.by((): Row[] => {
    const out: Row[] = [];
    for (const item of renderedItems) {
      const last = out.at(-1);
      if (isStep(item)) {
        if (last?.kind === 'steps') last.items.push(item);
        else out.push({ kind: 'steps', id: `steps:${item.id}`, items: [item] });
      } else {
        // A run of steps ends when the next message arrives.
        if (last?.kind === 'steps') last.end = item.timestamp;
        out.push({ kind: 'item', id: item.id, item });
      }
    }
    return out;
  });
  let expanded = $state<Set<string>>(new Set());
  function toggleGroup(id: string) {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id); else next.add(id);
    expanded = next;
  }

  type StepKind = 'edit' | 'command' | 'mcp' | 'script' | 'read' | 'search' | 'web' | 'thought' | 'other';
  const STEP_ICONS: Record<StepKind, typeof Brain> = {
    edit: Pencil, command: SquareTerminal, mcp: Plug, script: Code2, read: FileText, search: Search, web: Globe, thought: Brain, other: Puzzle,
  };
  function stepKind(item: ConversationItem): { kind: StepKind; server?: string; path?: string; failed?: boolean } {
    if (item.kind === 'thinking') return { kind: 'thought' };
    if (item.kind === 'custom') return { kind: item.customType === 'bashExecution' ? 'command' : 'other' };
    if (item.kind !== 'tool') return { kind: 'other' };
    const failed = item.status === 'failed' || item.result?.isError === true;
    const mcp = mcpCall(item);
    if (mcp) return { kind: mcp.kind === 'script' ? 'script' : 'mcp', server: mcp.server, failed };
    const name = item.toolName.toLowerCase();
    const path = typeof item.args.path === 'string' ? item.args.path : typeof item.args.file_path === 'string' ? item.args.file_path : undefined;
    if (/edit|write|patch|replace|create/.test(name)) return { kind: 'edit', path, failed };
    if (/bash|shell|exec|command|terminal|run/.test(name)) return { kind: 'command', failed };
    if (/web|fetch|url|browse/.test(name)) return { kind: 'web', failed };
    if (/grep|search|find|glob|^ls$|list/.test(name)) return { kind: 'search', failed };
    if (/read|view|cat|open/.test(name)) return { kind: 'read', path, failed };
    return { kind: 'other', failed };
  }
  function plural(count: number, one: string, many = `${one}s`): string {
    return `${count} ${count === 1 ? one : many}`;
  }
  function elapsed(items: ConversationItem[], end?: number): string | undefined {
    const times = [...items.map(item => item.timestamp), end].filter((t): t is number => typeof t === 'number' && t > 0);
    if (times.length < 2) return undefined;
    const seconds = Math.round((Math.max(...times) - Math.min(...times)) / 1000);
    if (seconds < 1) return undefined;
    return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
  }
  /** "Worked for 42s", the parts ("eureka-db ×3", "5 thoughts"), icons, and failures. */
  function summarize(items: ConversationItem[], end?: number) {
    const counts = new Map<StepKind, number>();
    const servers = new Map<string, number>();
    const edited = new Set<string>();
    const read = new Set<string>();
    let failed = 0;
    for (const item of items) {
      const step = stepKind(item);
      counts.set(step.kind, (counts.get(step.kind) ?? 0) + 1);
      if (step.failed) failed += 1;
      if (step.kind === 'mcp' && step.server) servers.set(step.server, (servers.get(step.server) ?? 0) + 1);
      if (step.kind === 'edit' && step.path) edited.add(step.path);
      if (step.kind === 'read' && step.path) read.add(step.path);
    }
    const n = (kind: StepKind) => counts.get(kind) ?? 0;
    const parts: string[] = [];
    if (n('edit')) parts.push(`edited ${plural(edited.size || n('edit'), 'file')}`);
    if (n('command')) parts.push(`ran ${plural(n('command'), 'command')}`);
    for (const [server, count] of servers) parts.push(count > 1 ? `${server} ×${count}` : server);
    const unnamedMcp = n('mcp') - [...servers.values()].reduce((a, b) => a + b, 0);
    if (unnamedMcp > 0) parts.push(plural(unnamedMcp, 'MCP call'));
    if (n('script')) parts.push(plural(n('script'), 'MCP script'));
    if (n('read')) parts.push(`read ${plural(read.size || n('read'), 'file')}`);
    if (n('search')) parts.push(plural(n('search'), 'search', 'searches'));
    if (n('web')) parts.push(plural(n('web'), 'web lookup'));
    if (n('other')) parts.push(plural(n('other'), 'other step'));
    if (n('thought')) parts.push(plural(n('thought'), 'thought'));
    const order: StepKind[] = ['edit', 'command', 'mcp', 'script', 'read', 'search', 'web', 'other', 'thought'];
    const icons = order.filter(kind => n(kind) > 0).slice(0, 3).map(kind => STEP_ICONS[kind]);
    const time = elapsed(items, end);
    return { title: time ? `Worked for ${time}` : 'Worked', parts: parts.slice(0, 4), more: parts.length > 4, icons, failed };
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

{#snippet entry(item: ConversationItem)}
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
        <span class="think-label" class:live={item.streaming}>{item.streaming ? 'Thinking…' : 'Thought'}</span>
        {#if !item.streaming && item.text}<span class="think-preview">{preview(item.text, 90)}</span>{/if}
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
{/snippet}

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
        data={rows}
        getKey={(row) => row.id}
        onscroll={onScroll}
        bufferSize={400}
        ssrCount={12}
      >
        {#snippet children(row, index)}
          {#if row.kind === 'item'}
            <div class="row kind-{row.item.kind}" class:first={index === 0} class:enter={isFresh(row.item.id)}>
              <div class="row-inner">{@render entry(row.item)}</div>
            </div>
          {:else}
            {@const live = (status === 'active' || status === 'waiting') && index === rows.length - 1}
            {@const foldable = row.items.length >= 3 && !live}
            {@const open = !foldable || expanded.has(row.id)}
            <div class="row steps-row" class:first={index === 0} class:enter={isFresh(row.items[0].id)}>
              <div class="row-inner">
                {#if foldable}
                  {@const summary = summarize(row.items, row.end)}
                  <button type="button" class="steps-summary" class:open aria-expanded={open} onclick={() => toggleGroup(row.id)}>
                    <span class="summary-icons" aria-hidden="true">
                      {#each summary.icons as Icon, i (i)}<span class="summary-icon"><Icon size={11} strokeWidth={2.1} /></span>{/each}
                    </span>
                    <span class="summary-title">{summary.title}</span>
                    <span class="summary-parts">
                      {#each summary.parts as part (part)}<span class="summary-part">{part}</span>{/each}
                      {#if summary.more}<span class="summary-part">…</span>{/if}
                    </span>
                    {#if summary.failed}<span class="summary-failed">{summary.failed} failed</span>{/if}
                    <span class="summary-chevron" aria-hidden="true"><ChevronRight size={13} strokeWidth={2} /></span>
                  </button>
                {/if}
                {#if open}
                  <div class="steps" class:folded={foldable} class:single={row.items.length === 1}>
                    {#each row.items as item (item.id)}
                      <div class="step" class:enter={isFresh(item.id)}>{@render entry(item)}</div>
                    {/each}
                  </div>
                {/if}
              </div>
            </div>
          {/if}
        {/snippet}
      </VList>
    </div>
    {#if status === 'active'}<WorkingLine {items} />{/if}

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
  .row.enter {
    animation: row-in 0.28s var(--ease) both;
  }
  @keyframes row-in {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
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
  .row.steps-row {
    padding: 4px 0 8px;
  }
  .steps {
    position: relative;
  }
  /* Timeline rail through the step nodes (node centre: 4px + 10px). */
  .steps::before {
    content: '';
    position: absolute;
    left: 13.5px;
    top: 14px;
    bottom: 14px;
    width: 1px;
    background: color-mix(in srgb, var(--subtle) 40%, transparent);
  }
  .steps.single::before {
    display: none;
  }
  .steps.folded {
    margin: 2px 0 2px;
    animation: row-in 0.22s var(--ease) both;
  }
  .step.enter {
    animation: row-in 0.28s var(--ease) both;
  }
  .steps-summary {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-width: 0;
    height: 32px;
    padding: 0 10px 0 4px;
    border: 0;
    border-radius: var(--radius);
    background: none;
    color: var(--muted);
    font-size: 12.5px;
    text-align: left;
    transition: background 0.12s;
  }
  .steps-summary:hover,
  .steps-summary.open {
    background: var(--surface);
  }
  .steps-summary:focus-visible {
    outline: none;
    box-shadow: var(--focus-ring);
  }
  .summary-icons {
    display: inline-flex;
    flex: none;
    padding-left: 6px;
  }
  .summary-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    margin-left: -6px;
    border-radius: 50%;
    background: var(--surface-2);
    box-shadow: 0 0 0 2px var(--bg);
    color: var(--muted);
  }
  .steps-summary:hover .summary-icon,
  .steps-summary.open .summary-icon {
    box-shadow: 0 0 0 2px var(--surface);
  }
  .summary-title {
    flex: none;
    color: var(--text);
    font-weight: 500;
  }
  .summary-parts {
    display: flex;
    gap: 6px;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
  }
  .summary-part {
    flex: none;
  }
  .summary-part + .summary-part::before {
    content: '·';
    margin-right: 6px;
    color: var(--subtle);
  }
  .summary-failed {
    flex: none;
    height: 18px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--bad-bg);
    color: var(--bad);
    font-size: 11px;
    font-weight: 600;
    line-height: 18px;
  }
  .summary-chevron {
    display: inline-flex;
    flex: none;
    margin-left: auto;
    color: var(--subtle);
    transition: transform 0.18s var(--ease);
  }
  .steps-summary.open .summary-chevron {
    transform: rotate(90deg);
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
    color: var(--subtle);
    font-weight: 500;
  }
  .think-label.live {
    background: linear-gradient(90deg, var(--subtle) 0%, var(--text) 50%, var(--subtle) 100%);
    background-size: 200% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    animation: think-shimmer 1.6s linear infinite;
  }
  @keyframes think-shimmer {
    to {
      background-position: -200% 0;
    }
  }
  .think-preview {
    margin-left: 8px;
    color: var(--subtle);
    font-style: italic;
    opacity: 0.85;
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
