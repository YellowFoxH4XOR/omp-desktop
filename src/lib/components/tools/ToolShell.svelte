<script lang="ts">
  import type { Component, Snippet } from 'svelte';
  import { ChevronRight, LoaderCircle, Wrench } from '@lucide/svelte';
  import type { ToolStatus } from '../../types';

  interface Props {
    status: ToolStatus;
    /** Muted metadata shown after the summary (duration, counts). */
    meta?: string;
    failed?: boolean;
    /** Category glyph drawn as the timeline node. */
    icon?: Component<{ size?: number; strokeWidth?: number }>;
    initiallyOpen?: boolean;
    /** Secondary steps (thinking, harness events) read quieter than tool calls. */
    quiet?: boolean;
    summary: Snippet;
    detail?: Snippet;
  }

  let { status, meta, failed = false, icon: Icon = Wrench, initiallyOpen = false, quiet = false, summary, detail }: Props = $props();

  // svelte-ignore state_referenced_locally
  let open = $state(initiallyOpen);

  const statusLabel: Record<ToolStatus, string> = {
    queued: 'Queued',
    running: 'Running',
    completed: 'Completed',
    failed: 'Failed',
    cancelled: 'Cancelled',
  };
</script>

<div
  class="tool"
  class:failed
  class:quiet
  class:open
  class:running={status === 'running'}
  class:queued={status === 'queued'}
  class:cancelled={status === 'cancelled'}
>
  <button
    type="button"
    class="head"
    onclick={() => detail && (open = !open)}
    aria-expanded={detail ? open : undefined}
  >
    <span class="sr-only">{failed ? 'Failed' : statusLabel[status]}</span>
    <span class="node" aria-hidden="true">
      {#if status === 'running'}
        <LoaderCircle size={12} strokeWidth={2.2} class="spin" />
      {:else}
        <Icon size={12} strokeWidth={2} />
      {/if}
    </span>
    <span class="summary">{@render summary()}</span>
    {#if meta}<span class="meta">{meta}</span>{/if}
    {#if detail}
      <span class="chevron" aria-hidden="true"><ChevronRight size={12} strokeWidth={2} /></span>
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
    position: relative;
    padding: 1px 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 28px;
    padding: 0 8px 0 4px;
    border: 0;
    border-radius: var(--radius);
    background: transparent;
    color: var(--text);
    font-size: 12.5px;
    text-align: left;
    min-width: 0;
    transition: background 0.12s;
  }
  .head:hover {
    background: var(--surface);
  }
  .node {
    position: relative;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 20px;
    height: 20px;
    border-radius: 6px;
    background: var(--bg);
    box-shadow: inset 0 0 0 1px var(--line-strong);
    color: var(--muted);
  }
  .head:hover .node {
    background: var(--surface);
  }
  .tool.running .node {
    color: var(--accent);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--accent) 45%, transparent);
    background: color-mix(in srgb, var(--accent) 10%, var(--bg));
  }
  .tool.failed .node {
    color: var(--bad);
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--bad) 45%, transparent);
    background: color-mix(in srgb, var(--bad) 10%, var(--bg));
  }
  .tool.queued .node,
  .tool.cancelled .node {
    color: var(--subtle);
  }
  .tool.quiet .node {
    box-shadow: none;
    color: var(--subtle);
  }
  .tool.quiet.running .node {
    color: var(--accent);
  }
  .summary {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
  }
  .summary :global(code),
  .summary :global(.c) {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text);
  }
  .tool.running .summary {
    color: var(--text);
  }
  .tool.cancelled .summary {
    text-decoration: line-through;
    text-decoration-color: var(--subtle);
  }
  .tool.failed .summary {
    color: var(--bad);
  }
  .meta {
    flex: none;
    color: var(--subtle);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .chevron {
    display: inline-flex;
    flex: none;
    color: var(--subtle);
    opacity: 0;
    transition: transform 0.15s var(--ease), opacity 0.12s;
  }
  .head:hover .chevron,
  .tool.open .chevron {
    opacity: 1;
  }
  .tool.open .chevron {
    transform: rotate(90deg);
  }
  .detail {
    margin: 4px 0 8px 34px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--surface);
    font-size: 12.5px;
    animation: ui-rise 0.14s var(--ease);
  }
  .tool.quiet .detail {
    background: transparent;
    border-color: transparent;
    padding: 2px 12px 4px 0;
    margin-top: 0;
  }
  .detail :global(pre) {
    margin: 6px 0;
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--bg);
    border: 1px solid var(--line);
    overflow-x: auto;
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 360px;
    overflow-y: auto;
  }
  .detail :global(pre:last-child) {
    margin-bottom: 0;
  }
  .detail :global(.cmd) {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text);
  }
  .detail :global(.err) {
    color: var(--bad);
  }
  .detail :global(.muted) {
    color: var(--muted);
  }
  .detail :global(.row) {
    display: flex;
    gap: 10px;
    align-items: baseline;
    min-width: 0;
    padding: 1px 0;
  }
  .detail :global(.k) {
    flex: none;
    min-width: 52px;
    color: var(--subtle);
    font-size: 11.5px;
  }
  .detail :global(.v) {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .detail :global(details.raw) {
    margin-top: 6px;
  }
  .detail :global(details.raw summary) {
    cursor: pointer;
    color: var(--muted);
    font-size: 11.5px;
  }
  .detail :global(.changes),
  .detail :global(.view) {
    margin-top: 8px;
    height: 26px;
    padding: 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--elevated);
    color: var(--text);
    font-size: 12px;
    font-weight: 500;
  }
  .detail :global(.changes:hover),
  .detail :global(.view:hover) {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
