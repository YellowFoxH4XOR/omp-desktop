<script lang="ts">
  import ToolShell from './ToolShell.svelte';
  import {
    argPath,
    countLines,
    detailNumber,
    formatDuration,
    preview,
    resultDuration,
    resultText,
    type ToolItem,
  } from './tool-utils';

  interface Props {
    item: ToolItem;
  }

  let { item }: Props = $props();

  const path = $derived(argPath(item.args) ?? item.toolName);
  const output = $derived(resultText(item.result ?? item.partial));
  const lines = $derived(
    detailNumber(item.result?.details, ['lines', 'lineCount', 'line_count']) ?? countLines(output),
  );
  const range = $derived.by(() => {
    const offset = item.args['offset'];
    const limit = item.args['limit'];
    if (offset === undefined && limit === undefined) return undefined;
    return `offset ${String(offset ?? 0)}${limit !== undefined ? ` · limit ${String(limit)}` : ''}`;
  });
  const duration = $derived(resultDuration(item.result));
  const meta = $derived(
    [
      lines > 0 ? `${lines} line${lines === 1 ? '' : 's'}` : undefined,
      duration !== undefined ? formatDuration(duration) : undefined,
    ]
      .filter(Boolean)
      .join(' · '),
  );
  const truncated = $derived(output.length > 3000);
  const shownOutput = $derived(truncated ? `${output.slice(0, 3000)}\n…` : output);
</script>

<ToolShell status={item.status} failed={item.status === 'failed' || item.result?.isError === true} meta={meta || undefined}>
  {#snippet summary()}
    <span class="line">Read <code class="c">{preview(path, 80)}</code></span>
  {/snippet}
  {#snippet detail()}
    <div class="row"><span class="k">path</span><span class="v">{path}</span></div>
    {#if range}
      <div class="row"><span class="k">range</span><span class="v muted">{range}</span></div>
    {/if}
    {#if shownOutput}
      <pre>{shownOutput}</pre>
    {:else if item.status === 'running'}
      <div class="muted">Reading…</div>
    {/if}
  {/snippet}
</ToolShell>

<style>
  .line {
    min-width: 0;
  }
  .c {
    font-family: var(--mono);
    font-size: 11.5px;
  }
</style>
