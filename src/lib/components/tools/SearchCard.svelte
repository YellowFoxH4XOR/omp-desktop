<script lang="ts">
  import { Search } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import {
    argPath,
    argQuery,
    detailNumber,
    formatDuration,
    preview,
    resultDuration,
    resultText,
    searchCounts,
    type ToolItem,
  } from './tool-utils';

  interface Props {
    item: ToolItem;
  }

  let { item }: Props = $props();

  const query = $derived(argQuery(item.args) ?? item.intent ?? '');
  const scope = $derived(argPath(item.args));
  const output = $derived(resultText(item.result ?? item.partial));
  const counts = $derived.by(() => {
    const details = item.result?.details;
    const matches = detailNumber(details, ['matches', 'matchCount', 'match_count', 'count']);
    const files = detailNumber(details, ['files', 'fileCount', 'file_count']);
    if (matches !== undefined || files !== undefined) {
      return { matches: matches ?? 0, files: files ?? 0 };
    }
    return searchCounts(output);
  });
  const duration = $derived(resultDuration(item.result));
  const meta = $derived(
    [
      counts && counts.matches > 0
        ? `${counts.matches} match${counts.matches === 1 ? '' : 'es'}`
        : undefined,
      counts && counts.files > 0 ? `${counts.files} file${counts.files === 1 ? '' : 's'}` : undefined,
      duration !== undefined ? formatDuration(duration) : undefined,
    ]
      .filter(Boolean)
      .join(' · '),
  );
  const truncated = $derived(output.length > 3000);
  const shownOutput = $derived(truncated ? `${output.slice(0, 3000)}\n…` : output);
</script>

<ToolShell icon={Search} status={item.status} failed={item.status === 'failed' || item.result?.isError === true} meta={meta || undefined}>
  {#snippet summary()}
    <span class="line">Searched <code class="c">{preview(query || item.toolName, 70)}</code></span>
  {/snippet}
  {#snippet detail()}
    {#if query}
      <div class="row"><span class="k">pattern</span><span class="v cmd">{query}</span></div>
    {/if}
    {#if scope}
      <div class="row"><span class="k">path</span><span class="v">{scope}</span></div>
    {/if}
    {#if shownOutput}
      <pre>{shownOutput}</pre>
    {:else if item.status === 'running'}
      <div class="muted">Searching…</div>
    {:else}
      <div class="muted">No matches</div>
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
