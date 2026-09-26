<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { ChevronDown, Cpu, Square, X } from '@lucide/svelte';
  import { api } from '$lib/api';
  import type { Project, RuntimeStats, ThreadRuntime } from '$lib/types';

  interface Props {
    projects: Project[];
    /** The thread shown in the main pane; bulk stop leaves it running. */
    activeThreadId: string | null;
    onStop: (threadIds: string[]) => Promise<void>;
    onOpenThread?: (threadId: string, projectId: string) => void;
  }

  let { projects, activeThreadId, onStop, onOpenThread }: Props = $props();

  const POLL_CLOSED_MS = 5_000;
  const POLL_OPEN_MS = 2_000;

  let stats = $state<RuntimeStats | null>(null);
  let failed = $state(false);
  let open = $state(false);
  let stopping = $state<Set<string>>(new Set());
  let root = $state<HTMLDivElement>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let destroyed = false;
  let inFlight = false;

  const threads = $derived(stats?.threads ?? []);
  const totalBytes = $derived(threads.reduce((sum, thread) => sum + (thread.memoryBytes ?? 0), 0));
  const busyCount = $derived(threads.filter((thread) => thread.busy).length);
  const idle = $derived(threads.filter((thread) => !thread.busy && thread.threadId !== activeThreadId && !stopping.has(thread.threadId)));
  const groups = $derived.by(() => {
    const byProject = new Map<string, ThreadRuntime[]>();
    for (const thread of threads) {
      const list = byProject.get(thread.projectId) ?? [];
      list.push(thread);
      byProject.set(thread.projectId, list);
    }
    return [...byProject.entries()]
      .map(([projectId, list]) => ({
        projectId,
        name: projects.find((project) => project.id === projectId)?.displayName ?? (projectId === 'pidesk-intern-project' ? 'Pi Intern' : 'Unknown project'),
        bytes: list.reduce((sum, thread) => sum + (thread.memoryBytes ?? 0), 0),
        threads: list,
      }))
      .sort((a, b) => b.bytes - a.bytes);
  });

  function formatBytes(bytes: number | null | undefined): string {
    if (bytes == null) return '—';
    const mb = bytes / (1024 * 1024);
    if (mb >= 1024) return `${(mb / 1024).toFixed(2)} GB`;
    return mb >= 100 ? `${Math.round(mb)} MB` : `${mb.toFixed(1)} MB`;
  }

  function formatIdle(seconds: number): string {
    if (seconds < 60) return 'idle <1m';
    if (seconds < 3600) return `idle ${Math.floor(seconds / 60)}m`;
    return `idle ${Math.floor(seconds / 3600)}h ${Math.floor((seconds % 3600) / 60)}m`;
  }

  async function refresh() {
    if (inFlight || destroyed) return;
    inFlight = true;
    try {
      stats = await api.getRuntimeStats();
      failed = false;
    } catch {
      failed = true;
    } finally {
      inFlight = false;
    }
  }

  function schedule() {
    clearTimeout(timer);
    if (destroyed) return;
    timer = setTimeout(async () => {
      if (document.visibilityState === 'visible') await refresh();
      schedule();
    }, open ? POLL_OPEN_MS : POLL_CLOSED_MS);
  }

  async function stop(ids: string[]) {
    if (ids.length === 0) return;
    stopping = new Set([...stopping, ...ids]);
    try {
      await onStop(ids);
    } finally {
      const next = new Set(stopping);
      for (const id of ids) next.delete(id);
      stopping = next;
      await refresh();
    }
  }

  function toggle() {
    open = !open;
    if (open) void refresh();
    schedule();
  }

  function onWindowPointer(event: PointerEvent) {
    if (open && root && !root.contains(event.target as Node)) {
      open = false;
      schedule();
    }
  }

  function onWindowKey(event: KeyboardEvent) {
    if (open && event.key === 'Escape') {
      event.stopPropagation();
      open = false;
      schedule();
    }
  }

  function onVisibility() {
    if (document.visibilityState === 'visible') void refresh();
  }

  onMount(() => {
    void refresh();
    schedule();
    document.addEventListener('visibilitychange', onVisibility);
  });

  onDestroy(() => {
    destroyed = true;
    clearTimeout(timer);
    document.removeEventListener('visibilitychange', onVisibility);
  });
</script>

<svelte:window onpointerdown={onWindowPointer} onkeydowncapture={onWindowKey} />

<div class="monitor" bind:this={root}>
  {#if open}
    <div class="panel" role="dialog" aria-label="Running Pi instances">
      <header>
        <div>
          <h3>Pi instances</h3>
          <p>{threads.length} running · {formatBytes(totalBytes)}</p>
        </div>
        <button type="button" class="icon" aria-label="Close runtime monitor" onclick={toggle}><X size={14} /></button>
      </header>

      <div class="body">
        {#if failed && !stats}
          <p class="empty">Couldn’t read process memory.</p>
        {:else if threads.length === 0}
          <p class="empty">No Pi instances are running. Open a thread to start one.</p>
        {:else}
          {#each groups as group (group.projectId)}
            <section>
              <div class="group-head">
                <span class="group-name">{group.name}</span>
                <span class="group-bytes">{formatBytes(group.bytes)}</span>
              </div>
              {#each group.threads as thread (thread.threadId)}
                {@const viewing = thread.threadId === activeThreadId}
                <div class="row" class:busy={thread.busy}>
                  <span class="dot" class:busy={thread.busy} aria-hidden="true"></span>
                  <button
                    type="button"
                    class="title"
                    title={`Open ${thread.title || 'New thread'}`}
                    onclick={() => {
                      onOpenThread?.(thread.threadId, thread.projectId);
                      open = false;
                      schedule();
                    }}
                  >
                    <span class="name">{thread.title || 'New thread'}</span>
                    <span class="meta">
                      {thread.busy ? 'Working' : formatIdle(thread.idleSeconds)}{viewing ? ' · open' : ''}{thread.processCount > 1 ? ` · ${thread.processCount} processes` : ''}
                    </span>
                  </button>
                  <span class="bytes">{formatBytes(thread.memoryBytes)}</span>
                  <button
                    type="button"
                    class="icon stop"
                    aria-label={`Stop ${thread.title || 'New thread'}`}
                    title={thread.busy ? 'Working — stop it from the thread instead' : 'Stop this Pi instance'}
                    disabled={thread.busy || stopping.has(thread.threadId)}
                    onclick={() => void stop([thread.threadId])}
                  >
                    <Square size={10} strokeWidth={0} fill="currentColor" />
                  </button>
                </div>
              {/each}
            </section>
          {/each}
        {/if}
      </div>

      <footer>
        <span class="app-bytes" title="πDesk's own backend process; the WebView renders separately">πDesk app {formatBytes(stats?.appBytes)}</span>
        <button type="button" class="stop-idle" disabled={idle.length === 0} onclick={() => void stop(idle.map((thread) => thread.threadId))}>
          Stop idle threads{idle.length ? ` (${idle.length})` : ''}
        </button>
      </footer>
    </div>
  {/if}

  <button
    type="button"
    class="pill"
    class:pressed={open}
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-label={`Pi runtime: ${threads.length} running, ${formatBytes(totalBytes)}`}
    onclick={toggle}
  >
    <span class="pill-dot" class:busy={busyCount > 0} class:none={threads.length === 0} aria-hidden="true"></span>
    <Cpu size={12} strokeWidth={2} />
    {#if threads.length === 0}
      <span>No Pi running</span>
    {:else}
      <span>{threads.length} Pi</span>
      <span class="sep" aria-hidden="true"></span>
      <span class="total">{formatBytes(totalBytes)}</span>
    {/if}
    <ChevronDown size={11} strokeWidth={2} class={open ? 'chev open' : 'chev'} />
  </button>
</div>

<style>
  .monitor {
    position: relative;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 24px;
    padding: 0 9px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: var(--surface);
    color: var(--muted);
    font-size: 11.5px;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
    transition: background 0.12s, color 0.12s;
  }
  .pill:hover,
  .pill.pressed {
    background: var(--surface-2);
    color: var(--text);
  }
  .pill :global(.chev) {
    color: var(--subtle);
    transform: rotate(180deg);
    transition: transform 0.15s var(--ease);
  }
  .pill :global(.chev.open) {
    transform: none;
  }
  .pill-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--good);
  }
  .pill-dot.busy {
    background: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-bg);
    animation: ui-pulse 1.8s ease-in-out infinite;
  }
  .pill-dot.none {
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--subtle);
  }
  .sep {
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: var(--subtle);
  }
  .total {
    color: var(--text);
  }
  .panel {
    position: absolute;
    right: 0;
    bottom: calc(100% + 8px);
    z-index: 25;
    width: 360px;
    max-height: min(480px, 70vh);
    display: flex;
    flex-direction: column;
    border-radius: var(--radius-lg);
    background: var(--elevated);
    box-shadow: var(--shadow);
    animation: ui-pop 0.14s var(--ease);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    padding: 12px 10px 10px 14px;
    border-bottom: 1px solid var(--line);
  }
  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
  }
  header p {
    margin: 2px 0 0;
    color: var(--subtle);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 6px;
  }
  .empty {
    margin: 0;
    padding: 18px 12px;
    color: var(--muted);
    font-size: 12px;
    text-align: center;
  }
  section + section {
    margin-top: 6px;
    padding-top: 6px;
    border-top: 1px solid var(--line);
  }
  .group-head {
    display: flex;
    justify-content: space-between;
    padding: 4px 8px 4px;
    font-size: 11.5px;
    font-weight: 600;
  }
  .group-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .group-bytes {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 4px 4px 8px;
    border-radius: var(--radius-sm);
  }
  .row:hover {
    background: var(--surface);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: transparent;
    box-shadow: inset 0 0 0 1.5px var(--subtle);
  }
  .dot.busy {
    background: var(--accent);
    box-shadow: none;
    animation: ui-pulse 1.8s ease-in-out infinite;
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 0;
    border: 0;
    background: transparent;
    text-align: left;
  }
  .name {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12.5px;
  }
  .meta {
    color: var(--subtle);
    font-size: 11px;
  }
  .row.busy .meta {
    color: var(--accent);
  }
  .bytes {
    flex: none;
    color: var(--muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--subtle);
  }
  .icon:hover:not(:disabled) {
    background: var(--surface-2);
    color: var(--text);
  }
  .icon.stop:hover:not(:disabled) {
    background: var(--bad-bg);
    color: var(--bad);
  }
  .icon:disabled {
    opacity: 0.35;
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px 8px 14px;
    border-top: 1px solid var(--line);
  }
  .app-bytes {
    color: var(--subtle);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .stop-idle {
    height: 26px;
    padding: 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--text);
    font-size: 12px;
    font-weight: 500;
  }
  .stop-idle:hover:not(:disabled) {
    border-color: color-mix(in srgb, var(--bad) 45%, var(--line-strong));
    color: var(--bad);
    background: var(--bad-bg);
  }
</style>
