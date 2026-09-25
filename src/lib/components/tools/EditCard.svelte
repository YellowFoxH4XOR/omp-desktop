<script lang="ts">
  import { FilePen, FilePlus2 } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import {
    argPath,
    detailNumber,
    detailString,
    formatDuration,
    preview,
    resultDuration,
    resultText,
    type ToolItem,
  } from './tool-utils';

  interface Props {
    item: ToolItem;
    /** True for write/create-style tools; false for edits. */
    write?: boolean;
    onShowChanges?: (path?: string) => void;
  }

  let { item, write = false, onShowChanges }: Props = $props();

  const path = $derived(argPath(item.args) ?? detailString(item.result?.details, ['path', 'file']) ?? '');
  const stats = $derived.by(() => {
    const details = item.result?.details;
    const additions = detailNumber(details, ['additions', 'added', 'insertions', 'linesAdded']);
    const deletions = detailNumber(details, ['deletions', 'removed', 'linesRemoved']);
    if (additions === undefined && deletions === undefined) return undefined;
    return { additions: additions ?? 0, deletions: deletions ?? 0 };
  });
  const output = $derived(resultText(item.result ?? item.partial));
  const duration = $derived(resultDuration(item.result));
  const verb = $derived(
    item.status === 'running' || item.status === 'queued'
      ? write
        ? 'Writing'
        : 'Editing'
      : write
        ? 'Wrote'
        : 'Edited',
  );
  const meta = $derived(
    [
      stats && stats.additions > 0 ? `+${stats.additions}` : undefined,
      stats && stats.deletions > 0 ? `−${stats.deletions}` : undefined,
      duration !== undefined ? formatDuration(duration) : undefined,
    ]
      .filter(Boolean)
      .join(' '),
  );
  const truncated = $derived(output.length > 2000);
  const shownOutput = $derived(truncated ? `${output.slice(0, 2000)}\n…` : output);
</script>

<ToolShell icon={write ? FilePlus2 : FilePen} status={item.status} failed={item.status === 'failed' || item.result?.isError === true} meta={meta || undefined}>
  {#snippet summary()}
    <span class="line">{verb} <code class="c">{preview(path || item.toolName, 80)}</code></span>
  {/snippet}
  {#snippet detail()}
    {#if path}
      <div class="row"><span class="k">path</span><span class="v">{path}</span></div>
    {/if}
    {#if item.intent}
      <div class="row"><span class="k">intent</span><span class="v muted">{item.intent}</span></div>
    {/if}
    {#if shownOutput}
      <pre>{shownOutput}</pre>
    {/if}
    {#if onShowChanges && path}
      <button type="button" class="changes" onclick={() => onShowChanges(path)}>
        View changes
      </button>
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
  .changes {
    margin-top: 4px;
    padding: 3px 10px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--accent);
    font-size: 11.5px;
  }
  .changes:hover {
    background: var(--surface-3);
  }
</style>
