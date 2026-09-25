<script lang="ts">
  import { onDestroy } from 'svelte';
  import { ChevronLeft, ChevronRight, X } from '@lucide/svelte';
  import { api } from '$lib/api';
  import { messagesToItems } from '$lib/session.svelte';
  import Transcript from '$lib/components/conversation/Transcript.svelte';
  import type { AgentInfo, ConversationItem, SessionView, ThreadStatus } from '$lib/types';

  interface Props {
    view: SessionView;
    onClose: () => void;
  }

  const { view, onClose }: Props = $props();

  const MAIN_ID = 'main';

  interface StatusFace {
    glyph: string;
    label: string;
    cls: 'accent' | 'good' | 'warn' | 'bad' | 'muted';
  }

  // Glyphs carry the meaning; color is only a reinforcement.
  const STATUS_FACE: Record<AgentInfo['status'], StatusFace> = {
    pending: { glyph: '○', label: 'Pending', cls: 'muted' },
    running: { glyph: '●', label: 'Working', cls: 'accent' },
    waiting: { glyph: '◉', label: 'Waiting', cls: 'warn' },
    completed: { glyph: '✓', label: 'Finished', cls: 'good' },
    failed: { glyph: '!', label: 'Failed', cls: 'bad' },
    aborted: { glyph: '■', label: 'Stopped', cls: 'bad' },
    parked: { glyph: '◌', label: 'Parked', cls: 'muted' },
  };

  function mainStatus(status: ThreadStatus): StatusFace {
    switch (status) {
      case 'active':
        return STATUS_FACE.running;
      case 'waiting':
        return STATUS_FACE.waiting;
      case 'idle':
        return { glyph: '◌', label: 'Idle', cls: 'muted' };
      case 'completed':
        return STATUS_FACE.completed;
      case 'failed':
        return STATUS_FACE.failed;
      case 'disconnected':
        return { ...STATUS_FACE.aborted, label: 'Disconnected' };
    }
  }

  // Advisor output stays in the conversation; it is not a controllable agent.
  function isAdvisor(agent: AgentInfo): boolean {
    return agent.role?.toLowerCase() === 'advisor' || agent.name.toLowerCase() === 'advisor';
  }

  function dottedParent(id: string): string | undefined {
    const dot = id.lastIndexOf('.');
    return dot > 0 ? id.slice(0, dot) : undefined;
  }

  interface AgentNode {
    agent: AgentInfo;
    children: AgentNode[];
  }

  interface Row {
    agent: AgentInfo;
    isMain: boolean;
    prefix: string;
  }

  // Harness agent ids feed keyed each blocks: dedupe first-wins so a duplicate
  // id can never throw each_key_duplicate.
  const subagents = $derived.by(() => {
    const seen = new Set<string>([MAIN_ID]);
    return view.agents.filter((a) => {
      if (a.id === MAIN_ID || isAdvisor(a) || seen.has(a.id)) return false;
      seen.add(a.id);
      return true;
    });
  });

  const mainAgent = $derived<AgentInfo>({
    id: MAIN_ID,
    name: 'Main',
    status:
      view.status === 'active'
        ? 'running'
        : view.status === 'waiting' || view.status === 'idle'
          ? 'waiting'
          : view.status === 'completed'
            ? 'completed'
            : view.status === 'failed'
              ? 'failed'
              : 'aborted',
    model: view.model?.name ?? view.model?.id,
    effort: view.effort,
    tokens: view.usage?.tokens.total,
    contextTokens: view.contextUsage?.tokens ?? view.usage?.contextUsage?.tokens ?? undefined,
    contextWindow: view.contextUsage?.contextWindow ?? view.usage?.contextUsage?.contextWindow,
  });

  // Build the hierarchy: explicit parentId wins, dotted-id prefix is the fallback,
  // anything unresolved hangs off the synthesized Main root.
  const roots = $derived.by(() => {
    const nodes = new Map<string, AgentNode>();
    for (const agent of subagents) nodes.set(agent.id, { agent, children: [] });

    const top: AgentNode[] = [];
    for (const node of nodes.values()) {
      const parentId = node.agent.parentId ?? dottedParent(node.agent.id);
      const parent = parentId ? nodes.get(parentId) : undefined;
      const seen = new Set([node.agent.id]);
      let cursor = parentId;
      let cyclic = false;
      while (cursor && nodes.has(cursor)) {
        if (seen.has(cursor)) { cyclic = true; break; }
        seen.add(cursor);
        const ancestor = nodes.get(cursor)!.agent;
        cursor = ancestor.parentId ?? dottedParent(ancestor.id);
      }
      if (parent && !cyclic) parent.children.push(node);
      else top.push(node);
    }
    return top;
  });

  const rows = $derived.by(() => {
    const out: Row[] = [{ agent: mainAgent, isMain: true, prefix: '' }];
    const seen = new Set<string>([MAIN_ID]);
    const walk = (nodes: AgentNode[], guides: string) => {
      nodes.forEach((node, i) => {
        // Duplicate harness agent ids would throw each_key_duplicate; first wins.
        if (seen.has(node.agent.id)) return;
        seen.add(node.agent.id);
        const last = i === nodes.length - 1;
        out.push({ agent: node.agent, isMain: false, prefix: `${guides}${last ? '└─' : '├─'}` });
        walk(node.children, `${guides}${last ? '  ' : '│ '}`);
      });
    };
    walk(roots, '');
    return out;
  });

  function statusOf(row: Row): StatusFace {
    return row.isMain ? mainStatus(view.status) : STATUS_FACE[row.agent.status];
  }

  // Agent progress events carry durationMs, so runtime display is event-driven
  // and does not keep a panel timer alive for queued or long-running agents.
  function runtimeMs(agent: AgentInfo): number | undefined {
    return agent.durationMs;
  }

  // --- Formatting -----------------------------------------------------------
  function formatTokens(n: number | undefined): string | undefined {
    if (n == null) return undefined;
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return String(n);
  }

  function formatDuration(ms: number | undefined): string | undefined {
    if (ms == null || ms < 0) return undefined;
    const s = Math.floor(ms / 1000);
    if (s < 60) return `${s}s`;
    const m = Math.floor(s / 60);
    if (m < 60) return s % 60 ? `${m}m ${s % 60}s` : `${m}m`;
    const h = Math.floor(m / 60);
    return `${h}h ${m % 60}m`;
  }

  function metaParts(row: Row): string[] {
    const { agent } = row;
    const parts: string[] = [];
    if (agent.role && !row.isMain) parts.push(agent.role);
    const model = agent.model && agent.effort ? `${agent.model} · ${agent.effort}` : (agent.model ?? agent.effort);
    if (model) parts.push(model);
    const tokens = formatTokens(agent.tokens);
    if (tokens) parts.push(`${tokens} tok`);
    if (agent.contextTokens != null) {
      const used = formatTokens(agent.contextTokens);
      parts.push(agent.contextWindow ? `ctx ${used}/${formatTokens(agent.contextWindow)}` : `ctx ${used}`);
    }
    const runtime = formatDuration(runtimeMs(agent));
    if (runtime) parts.push(runtime);
    if (agent.toolCount != null) parts.push(`${agent.toolCount} tools`);
    return parts;
  }

  // --- Selection + transcript ----------------------------------------------
  const MAX_DETAIL_ITEMS = 5_000;
  const REFRESH_INTERVAL_MS = 1_000;
  let selectedId = $state<string | null>(null);
  // Replaced wholesale on every fetch and never mutated: skip deep proxying.
  let detailItems = $state.raw<ConversationItem[]>([]);
  let detailLoading = $state(false);
  let detailError = $state<string | null>(null);
  let fetchSeq = 0;
  let progressTimer: ReturnType<typeof setTimeout> | undefined;
  // Non-reactive: only compared inside the refresh effect.
  let lastFingerprint = '';

  const selected = $derived(
    selectedId === MAIN_ID ? mainAgent : (subagents.find((a) => a.id === selectedId) ?? null),
  );

  function fingerprintOf(agent: AgentInfo): string {
    return `${agent.id}|${agent.status}|${agent.activity ?? ''}|${agent.toolCount ?? ''}|${agent.tokens ?? ''}|${agent.durationMs ?? ''}`;
  }

  async function loadTranscript(agentId: string) {
    const seq = ++fetchSeq;
    detailLoading = true;
    detailError = null;
    try {
      const messages = await api.getSubagentMessages(view.threadId, agentId);
      if (seq !== fetchSeq) return;
      detailItems = messagesToItems(messages.slice(-MAX_DETAIL_ITEMS));
    } catch (e) {
      if (seq !== fetchSeq) return;
      detailItems = [];
      detailError = e instanceof Error ? e.message : String(e);
    } finally {
      if (seq === fetchSeq) detailLoading = false;
    }
  }

  // Event-driven refresh: re-fetch the open transcript when the agent's
  // progress fields change. Throttled, not debounced: a running agent emits
  // progress continuously, and a resetting debounce would never fire.
  let firstLoadPending = true;
  $effect(() => {
    const sel = selected;
    if (!sel || sel.id === MAIN_ID) {
      clearTimeout(progressTimer);
      progressTimer = undefined;
      return;
    }
    const threadId = view.threadId;
    const fingerprint = fingerprintOf(sel);
    if (fingerprint === lastFingerprint) return;
    lastFingerprint = fingerprint;
    if (progressTimer !== undefined) return;
    const delay = firstLoadPending ? 0 : REFRESH_INTERVAL_MS;
    firstLoadPending = false;
    progressTimer = setTimeout(() => {
      progressTimer = undefined;
      const current = selected;
      if (view.threadId === threadId && current && current.id !== MAIN_ID) void loadTranscript(current.id);
    }, delay);
  });

  // Reset when the panel is reused for a different thread. Invalidate any
  // in-flight request before clearing the transcript it could still update.
  $effect(() => {
    void view.threadId;
    fetchSeq += 1;
    resetRefresh();
    selectedId = null;
    detailItems = [];
    detailError = null;
    detailLoading = false;
    lastFingerprint = '';
  });

  function resetRefresh() {
    clearTimeout(progressTimer);
    progressTimer = undefined;
    firstLoadPending = true;
  }

  function select(id: string) {
    // Invalidate the previous fetch now so it cannot populate this selection
    // before the replacement fetch starts.
    fetchSeq += 1;
    resetRefresh();
    selectedId = id;
    detailItems = [];
    detailError = null;
    detailLoading = id !== MAIN_ID;
    lastFingerprint = '';
  }

  function deselect() {
    fetchSeq += 1;
    resetRefresh();
    selectedId = null;
    detailItems = [];
    detailError = null;
    detailLoading = false;
    lastFingerprint = '';
  }
  function retry() {
    const sel = selected;
    if (!sel || sel.id === MAIN_ID) return;
    lastFingerprint = '';
    void loadTranscript(sel.id);
  }

  onDestroy(() => {
    fetchSeq += 1;
    clearTimeout(progressTimer);
  });
</script>

<aside class="panel" aria-label="Agents">
  <header class="head">
    <h2>Agents</h2>
    {#if subagents.length > 0}
      <span class="count">{subagents.length}</span>
    {/if}
    <span class="grow"></span>
    <button type="button" class="icon-btn" onclick={onClose} aria-label="Close agents panel" title="Close agents panel"><X size={15} /></button>
  </header>

  {#if selected}
    {@const st = statusOf({ agent: selected, isMain: selected.id === MAIN_ID, prefix: '' })}
    {@const parts = metaParts({ agent: selected, isMain: selected.id === MAIN_ID, prefix: '' })}
    <div class="detail">
      <div class="detail-head">
        <button type="button" class="back" onclick={deselect}><ChevronLeft size={13} strokeWidth={2} /> All agents</button>
        <div class="detail-title">
          <span class="dot {st.cls}" aria-hidden="true"></span>
          <span class="name">{selected.id === MAIN_ID ? 'Main agent' : selected.name}</span>
          <span class="status-chip {st.cls}">{st.label}</span>
        </div>
        {#if selected.task}
          <p class="task">{selected.task}</p>
        {/if}
        {#if parts.length}
          <div class="chips">
            {#each parts as part, chipIndex (chipIndex + ':' + part)}
              <span class="chip">{part}</span>
            {/each}
          </div>
        {/if}
        {#if selected.worktreePath}
          <div class="chips">
            <span class="chip path" title={selected.worktreePath}>{selected.worktreePath}</span>
          </div>
        {/if}
      </div>

      <div class="transcript-wrap">
        {#if selected.id === MAIN_ID}
          <Transcript items={view.items} agents={view.agents} onShowAgents={deselect} />
        {:else if detailError}
          <div class="state">
            <p class="state-title">Couldn't load this agent's transcript.</p>
            <p class="state-msg">{detailError}</p>
            <button type="button" class="retry" onclick={retry}>Retry</button>
          </div>
        {:else if detailLoading && detailItems.length === 0}
          <div class="state">
            <p class="state-title">Loading transcript…</p>
          </div>
        {:else if detailItems.length === 0}
          <div class="state">
            <p class="state-title">No messages yet.</p>
            <p class="state-msg">This agent hasn't produced any visible output.</p>
          </div>
        {:else}
          <Transcript items={detailItems} agents={view.agents} onShowAgents={deselect} />
        {/if}
      </div>
    </div>
  {:else}
    <div class="tree" role="tree">
      {#each rows as row, rowIndex (rowIndex + ':' + (row.isMain ? MAIN_ID : row.agent.id))}
        {@const st = statusOf(row)}
        {@const parts = metaParts(row)}
        {@const depth = row.prefix.length / 2}
        <button
          type="button"
          class="row"
          class:main={row.isMain}
          class:nested={depth > 0}
          role="treeitem"
          aria-selected="false"
          aria-level={depth + 1}
          style={`--depth:${depth}`}
          onclick={() => select(row.agent.id)}
        >
          <span class="dot {st.cls}" aria-hidden="true"></span>
          <span class="row-body">
            <span class="row-top">
              <span class="name">{row.isMain ? 'Main agent' : row.agent.name}</span>
              <span class="status-chip {st.cls}">{st.label}</span>
            </span>
            {#if row.agent.task}
              <span class="task">{row.agent.task}</span>
            {/if}
            {#if row.agent.activity && st.cls === 'accent'}
              <span class="activity">{row.agent.activity}</span>
            {/if}
            {#if parts.length}
              <span class="meta">{parts.join(' · ')}</span>
            {/if}
          </span>
          <ChevronRight size={13} strokeWidth={2} class="row-chev" />
        </button>
      {/each}
      {#if subagents.length === 0}
        <p class="empty">No subagents yet. Delegated work will appear here.</p>
      {/if}
    </div>
  {/if}
</aside>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--panel);
    color: var(--text);
    font-size: 12.5px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--header-height);
    min-height: var(--header-height);
    padding: 0 10px 0 16px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .head h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  .grow {
    flex: 1;
  }
  .count {
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 9px;
    background: var(--surface-2);
    color: var(--muted);
    font-size: 11px;
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--muted);
    cursor: pointer;
  }
  .icon-btn:hover {
    background: var(--surface-2);
    color: var(--text);
  }

  /* --- Tree --- */
  .tree {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 10px;
    width: 100%;
    padding: 9px 10px 9px calc(12px + var(--depth) * 16px);
    border: none;
    border-radius: var(--radius);
    background: none;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
    transition: background 0.12s;
  }
  .row.nested::before {
    content: '';
    position: absolute;
    left: calc(15px + (var(--depth) - 1) * 16px);
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--line-strong);
  }
  .row:hover {
    background: var(--surface);
  }
  .row.main {
    background: var(--surface);
    border: 1px solid var(--line);
    margin-bottom: 4px;
  }
  .row.main:hover {
    background: var(--surface-2);
  }
  .row :global(.row-chev) {
    flex: none;
    margin-top: 2px;
    color: var(--subtle);
    opacity: 0;
    transition: opacity 0.12s;
  }
  .row:hover :global(.row-chev) {
    opacity: 1;
  }
  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    margin-top: 5px;
    border-radius: 50%;
    background: var(--subtle);
  }
  .dot.accent {
    background: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-bg);
    animation: ui-pulse 1.8s ease-in-out infinite;
  }
  .dot.good {
    background: var(--good);
  }
  .dot.warn {
    background: var(--warn);
    box-shadow: 0 0 0 3px var(--warn-bg);
  }
  .dot.bad {
    background: var(--bad);
  }
  .dot.muted {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--subtle);
  }
  .row-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .name {
    font-weight: 600;
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .status-chip {
    flex: none;
    padding: 1px 7px;
    border-radius: 999px;
    font-size: 10.5px;
    font-weight: 500;
    color: var(--muted);
    background: var(--surface-2);
  }
  .status-chip.accent {
    color: var(--accent);
    background: var(--accent-bg);
  }
  .status-chip.good {
    color: var(--good);
    background: var(--good-bg);
  }
  .status-chip.warn {
    color: var(--warn);
    background: var(--warn-bg);
  }
  .status-chip.bad {
    color: var(--bad);
    background: var(--bad-bg);
  }
  .task {
    color: var(--muted);
    line-height: 1.45;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .meta {
    color: var(--subtle);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .activity {
    color: var(--accent);
    font-size: 11.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .empty {
    margin: 12px 10px;
    color: var(--subtle);
    font-size: 12px;
    text-align: center;
  }

  /* --- Detail --- */
  .detail {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .detail-head {
    flex: none;
    padding: 10px 16px 14px;
    border-bottom: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .back {
    align-self: flex-start;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 24px;
    margin-left: -6px;
    padding: 0 8px 0 4px;
    border: none;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--muted);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .back:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .detail-title {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .detail-title .dot {
    margin-top: 0;
  }
  .detail-title .name {
    font-size: 15px;
  }
  .detail-head .task {
    margin: 0;
    color: var(--text);
    -webkit-line-clamp: 4;
    line-clamp: 4;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .chip {
    background: var(--surface-2);
    color: var(--muted);
    border-radius: 5px;
    padding: 2px 7px;
    font-size: 11px;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip.path {
    font-family: var(--mono);
  }
  .transcript-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    --content-width: 100%;
  }
  .state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 20px;
    text-align: center;
  }
  .state-title {
    margin: 0;
    color: var(--text);
    font-weight: 500;
  }
  .state-msg {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
    word-break: break-word;
  }
  .retry {
    margin-top: 6px;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--elevated);
    color: var(--text);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .retry:hover {
    background: var(--surface-2);
  }
</style>
