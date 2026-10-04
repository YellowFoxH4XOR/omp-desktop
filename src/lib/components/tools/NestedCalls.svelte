<script lang="ts">
  import { CircleCheck, CircleSlash, CircleX } from '@lucide/svelte';
  import type { NestedCall } from '../../types';
  import { argPills, humanize, mcpToolName } from './mcp-tools.svelte';
  import { formatDuration } from './tool-utils';

  let { calls }: { calls: NestedCall[] } = $props();

  function label(call: NestedCall): string {
    const mapped = mcpToolName(call.toolName);
    return mapped ? `${mapped.server} · ${humanize(mapped.tool)}` : call.toolName;
  }
</script>

<div class="section">Calls</div>
<div class="calls">
  {#each calls as call (call.id)}
    <div class="call" class:bad={call.status === 'failed'}>
      <span class="state state-{call.status}" aria-hidden="true">
        {#if call.status === 'running'}
          <span class="dot"></span>
        {:else if call.status === 'completed'}
          <CircleCheck size={11} strokeWidth={2.2} />
        {:else if call.status === 'failed'}
          <CircleX size={11} strokeWidth={2.2} />
        {:else}
          <CircleSlash size={11} strokeWidth={2.2} />
        {/if}
      </span>
      <span class="label">{label(call)}</span>
      {#each argPills(call.args) as [key, value] (key)}<span class="pill"><span class="pk">{key}</span> {value}</span>{/each}
      {#if call.durationMs !== undefined}<span class="time">{formatDuration(call.durationMs)}</span>{/if}
    </div>
    {#if call.status === 'failed' && call.error}
      <div class="err">{call.error}</div>
    {/if}
  {/each}
</div>

<style>
  .section {
    margin: 12px 0 6px;
    color: var(--subtle);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
  }
  .calls {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .call {
    display: flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    font-size: 12px;
  }
  .state {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 14px;
    height: 14px;
    color: var(--subtle);
  }
  .state-running {
    color: var(--accent);
  }
  .state-completed {
    color: var(--good);
  }
  .state-failed {
    color: var(--bad);
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
  }
  .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text);
  }
  .call.bad .label {
    color: var(--bad);
  }
  .pill {
    flex: none;
    max-width: 180px;
    overflow: hidden;
    text-overflow: ellipsis;
    height: 19px;
    padding: 0 7px;
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--text);
    font: 11px/19px var(--mono);
  }
  .pk {
    color: var(--subtle);
    font-family: var(--font);
  }
  .time {
    flex: none;
    margin-left: auto;
    color: var(--subtle);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .err {
    margin: 1px 0 2px 21px;
    color: var(--bad);
    font-size: 11.5px;
    overflow-wrap: anywhere;
  }
</style>
