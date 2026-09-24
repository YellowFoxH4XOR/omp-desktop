<script lang="ts">
  import type { Snippet } from 'svelte';
  import {
    Check,
    ChevronDown,
    ChevronRight,
    Circle,
    CircleX,
    LoaderCircle,
    Octagon,
  } from '@lucide/svelte';
  import type { ToolStatus } from '../../types';

  interface Props {
    status: ToolStatus;
    /** Muted metadata shown after the summary (duration, counts). */
    meta?: string;
    failed?: boolean;
    summary: Snippet;
    detail?: Snippet;
  }

  let { status, meta, failed = false, summary, detail }: Props = $props();

  let open = $state(false);

  const statusLabel: Record<ToolStatus, string> = {
    queued: 'Queued',
    running: 'Running',
    completed: 'Completed',
    failed: 'Failed',
    cancelled: 'Cancelled',
  };
</script>

<div class="tool" class:failed class:running={status === 'running'}>
  <button
    type="button"
    class="head"
    onclick={() => detail && (open = !open)}
    aria-expanded={detail ? open : undefined}
  >
    <span class="sr-only">{statusLabel[status]}</span>
    <span class="icon" aria-hidden="true">
      {#if status === 'running'}
        <LoaderCircle size={13} class="spin" />
      {:else if status === 'completed'}
        <Check size={13} />
      {:else if status === 'failed'}
        <CircleX size={13} />
      {:else if status === 'cancelled'}
        <Octagon size={12} />
      {:else}
        <Circle size={11} />
      {/if}
    </span>
    <span class="summary">{@render summary()}</span>
    {#if meta}<span class="meta">{meta}</span>{/if}
    {#if detail}
      <span class="chevron" aria-hidden="true">
        {#if open}<ChevronDown size={12} />{:else}<ChevronRight size={12} />{/if}
      </span>
    {/if}
  </button>
  {#if detail && open}
    <div class="detail">{@render detail()}</div>
  {/if}
</div>
<style>
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

  .tool {
    margin: 3px 0;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    padding: 5px 8px;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 12px;
    text-align: left;
    min-width: 0;
  }
  .head:hover {
    background: var(--surface-2);
  }
  .icon {
    display: inline-flex;
    flex: none;
    color: var(--muted);
  }
  .tool.running .icon {
    color: var(--accent);
  }
  .tool.failed .icon {
    color: var(--bad);
  }
  .tool:not(.failed):not(.running) .icon {
    color: var(--good);
  }
  .tool .icon :global(.spin) {
    animation: spin 1s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .summary {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    flex: none;
    color: var(--muted);
    font-size: 11px;
  }
  .chevron {
    display: inline-flex;
    flex: none;
    color: var(--muted);
  }
  .detail {
    border-top: 1px solid var(--line);
    padding: 6px 8px;
    font-size: 12px;
  }
  .detail :global(pre) {
    margin: 4px 0;
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--bg);
    overflow-x: auto;
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .detail :global(.cmd) {
    color: var(--accent);
  }
  .detail :global(.err) {
    color: var(--bad);
  }
  .detail :global(.muted) {
    color: var(--muted);
  }
  .detail :global(.row) {
    display: flex;
    gap: 6px;
    align-items: baseline;
    min-width: 0;
  }
  .detail :global(.k) {
    flex: none;
    color: var(--muted);
    font-size: 11px;
  }
  .detail :global(.v) {
    min-width: 0;
    overflow-wrap: anywhere;
  }
</style>
