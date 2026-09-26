<script lang="ts">
  import { Wrench } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import {
    formatDuration,
    prettyJson,
    preview,
    resultDuration,
    resultText,
    type ToolItem,
  } from './tool-utils';

  interface Props {
    item: ToolItem;
  }

  let { item }: Props = $props();

  const output = $derived(resultText(item.result ?? item.partial));
  const duration = $derived(resultDuration(item.result));
  const meta = $derived(duration !== undefined ? formatDuration(duration) : undefined);
  const truncated = $derived(output.length > 3000);
  const shownOutput = $derived(truncated ? `${output.slice(0, 3000)}\n…` : output);
  const hasArgs = $derived(Object.keys(item.args).length > 0);
  const MAX_DUMP_CHARS = 20_000;
</script>

<ToolShell icon={Wrench} status={item.status} failed={item.status === 'failed' || item.result?.isError === true} {meta}>
  {#snippet summary()}
    <span class="line">
      {item.toolName}
      {#if item.intent}<span class="t">— {preview(item.intent, 60)}</span>{/if}
    </span>
  {/snippet}
  {#snippet detail()}
    {#if item.intent}
      <div class="row"><span class="k">intent</span><span class="v muted">{item.intent}</span></div>
    {/if}
    {#if shownOutput}
      <pre>{shownOutput}</pre>
    {:else if item.status === 'running'}
      <div class="muted">Running…</div>
    {/if}
    {#if hasArgs}
      <details class="raw">
        <summary>Arguments</summary>
        <pre>{prettyJson(item.args, MAX_DUMP_CHARS)}</pre>
      </details>
    {/if}
    {#if item.result?.details !== undefined}
      <details class="raw">
        <summary>Result details</summary>
        <pre>{prettyJson(item.result.details, MAX_DUMP_CHARS)}</pre>
      </details>
    {/if}
  {/snippet}
</ToolShell>

<style>
  .line {
    min-width: 0;
  }
  .t {
    color: var(--muted);
  }
  .raw {
    margin-top: 4px;
  }
  .raw summary {
    cursor: pointer;
    color: var(--muted);
    font-size: 11px;
  }
  .raw summary:hover {
    color: var(--text);
  }
</style>
