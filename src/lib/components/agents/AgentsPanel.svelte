<script lang="ts">
  import { onDestroy } from 'svelte';
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

  const subagents = $derived(view.agents.filter((a) => a.id !== MAIN_ID && !isAdvisor(a)));

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
    const walk = (nodes: AgentNode[], guides: string) => {
      nodes.forEach((node, i) => {
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
  let selectedId = $state<string | null>(null);
  let detailItems = $state<ConversationItem[]>([]);
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
  // progress fields change; coalesce bursts into one request per window.
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
    clearTimeout(progressTimer);
    progressTimer = setTimeout(() => {
      progressTimer = undefined;
      if (view.threadId === threadId && selected?.id === sel.id) void loadTranscript(sel.id);
    }, 250);
  });

  // Reset when the panel is reused for a different thread. Invalidate any
  // in-flight request before clearing the transcript it could still update.
  $effect(() => {
    void view.threadId;
    fetchSeq += 1;
    selectedId = null;
    detailItems = [];
    detailError = null;
    detailLoading = false;
    lastFingerprint = '';
  });

  function select(id: string) {
    // The replacement fetch starts after debounce; invalidate the previous one
    // now so it cannot populate this selection during that window.
    fetchSeq += 1;
    selectedId = id;
    detailItems = [];
    detailError = null;
    detailLoading = id !== MAIN_ID;
    lastFingerprint = '';
  }

  function deselect() {
    fetchSeq += 1;
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
    <button type="button" class="icon-btn" onclick={onClose} aria-label="Close agents panel">×</button>
  </header>

  {#if selected}
    {@const st = statusOf({ agent: selected, isMain: selected.id === MAIN_ID, prefix: '' })}
    {@const parts = metaParts({ agent: selected, isMain: selected.id === MAIN_ID, prefix: '' })}
    <div class="detail">
      <div class="detail-head">
        <button type="button" class="back" onclick={deselect}>← Agents</button>
        <div class="detail-title">
          <span class="glyph {st.cls}" aria-hidden="true">{st.glyph}</span>
          <div class="detail-name">
            <span class="name">{selected.id === MAIN_ID ? 'Main agent' : selected.name}</span>
            <span class="status-label">{st.label}</span>
          </div>
        </div>
        {#if selected.task}
          <p class="task">{selected.task}</p>
        {/if}
        {#if parts.length}
          <div class="chips">
            {#each parts as part (part)}
              <span class="chip">{part}</span>
            {/each}
          </div>
        {/if}
        {#if selected.worktreePath}
          <div class="chips">
            <span class="chip path" title={selected.worktreePath}>⎇ {selected.worktreePath}</span>
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
      {#each rows as row (row.isMain ? MAIN_ID : row.agent.id)}
        {@const st = statusOf(row)}
        {@const parts = metaParts(row)}
        <button
          type="button"
          class="row"
          class:main={row.isMain}
          role="treeitem"
          aria-selected="false"
          onclick={() => select(row.agent.id)}
        >
          {#if row.prefix}
            <span class="guide" aria-hidden="true">{row.prefix}</span>
          {/if}
          <span class="glyph {st.cls}" aria-hidden="true">{st.glyph}</span>
          <span class="row-body">
            <span class="row-top">
              <span class="name">{row.isMain ? 'Main agent' : row.agent.name}</span>
              <span class="status-label">{st.label}</span>
            </span>
            {#if row.agent.task}
              <span class="task">{row.agent.task}</span>
            {/if}
            {#if parts.length}
              <span class="meta">{parts.join(' · ')}</span>
            {/if}
            {#if row.agent.activity}
              <span class="activity">{row.agent.activity}</span>
            {/if}
          </span>
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
    background: var(--surface);
    border-left: 1px solid var(--line);
    color: var(--text);
    font-size: 12px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }

  .head h2 {
    flex: 1;
    margin: 0;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted);
  }

  .count {
    background: var(--surface-2);
    color: var(--muted);
    border-radius: 8px;
    padding: 0 6px;
    font-size: 10px;
    line-height: 16px;
  }

  .icon-btn {
    background: none;
    border: none;
    color: var(--muted);
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 3px 6px;
    border-radius: 4px;
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
    padding: 6px;
  }

  .row {
    display: flex;
    align-items: flex-start;
    gap: 4px;
    width: 100%;
    padding: 6px 8px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .row:hover {
    background: var(--surface-2);
  }

  .guide {
    flex: none;
    color: var(--muted);
    font-family: ui-monospace, monospace;
    white-space: pre;
    opacity: 0.7;
  }

  .glyph {
    flex: none;
    width: 14px;
    text-align: center;
  }

  .glyph.accent {
    color: var(--accent);
  }

  .glyph.good {
    color: var(--good);
  }

  .glyph.warn {
    color: var(--warn);
  }

  .glyph.bad {
    color: var(--bad);
  }

  .glyph.muted {
    color: var(--muted);
  }

  .row-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .row-top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row.main .name {
    color: var(--accent);
  }

  .status-label {
    flex: none;
    font-size: 10px;
    color: var(--muted);
  }

  .task {
    color: var(--text);
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .meta {
    color: var(--muted);
    font-size: 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .activity {
    color: var(--accent);
    font-size: 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .empty {
    margin: 4px 8px;
    color: var(--muted);
    font-size: 11px;
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
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .back {
    align-self: flex-start;
    background: none;
    border: none;
    color: var(--muted);
    font: inherit;
    font-size: 11px;
    cursor: pointer;
    padding: 0;
  }

  .back:hover {
    color: var(--text);
  }

  .detail-title {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .detail-name {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }

  .detail-name .name {
    font-size: 13px;
  }

  .detail-head .task {
    margin: 0;
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
    border-radius: 4px;
    padding: 1px 6px;
    font-size: 10px;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip.path {
    font-family: ui-monospace, monospace;
  }

  .transcript-wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .state {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 16px;
    text-align: center;
  }

  .state-title {
    margin: 0;
    color: var(--text);
  }

  .state-msg {
    margin: 0;
    color: var(--muted);
    font-size: 11px;
    word-break: break-word;
  }

  .retry {
    margin-top: 4px;
    background: var(--surface-2);
    border: 1px solid var(--line);
    color: var(--text);
    border-radius: 6px;
    padding: 4px 12px;
    font: inherit;
    font-size: 11px;
    cursor: pointer;
  }

  .retry:hover {
    border-color: var(--accent);
  }
</style>
