<script lang="ts">
  import { ExternalLink, Globe } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import { openExternal } from '../conversation/links';
  import {
    argQuery,
    argUrl,
    extractUrls,
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

  const query = $derived(argQuery(item.args) ?? argUrl(item.args) ?? item.intent ?? '');
  const output = $derived(resultText(item.result ?? item.partial));
  const urls = $derived(extractUrls(item.args, output));
  const duration = $derived(resultDuration(item.result));
  const meta = $derived(
    [
      urls.length > 0 ? `${urls.length} source${urls.length === 1 ? '' : 's'}` : undefined,
      duration !== undefined ? formatDuration(duration) : undefined,
    ]
      .filter(Boolean)
      .join(' · '),
  );
  const truncated = $derived(output.length > 3000);
  const shownOutput = $derived(truncated ? `${output.slice(0, 3000)}\n…` : output);
</script>

<ToolShell icon={Globe} status={item.status} failed={item.status === 'failed' || item.result?.isError === true} meta={meta || undefined}>
  {#snippet summary()}
    <span class="line">Web <code class="c">{preview(query || item.toolName, 70)}</code></span>
  {/snippet}
  {#snippet detail()}
    {#if query}
      <div class="row"><span class="k">query</span><span class="v">{query}</span></div>
    {/if}
    {#if urls.length > 0}
      <div class="sources">
        {#each urls as url, urlIndex (urlIndex + ':' + url)}
          <button type="button" class="source" onclick={() => void openExternal(url)}>
            <ExternalLink size={11} />
            <span class="u">{url}</span>
          </button>
        {/each}
      </div>
    {/if}
    {#if shownOutput}
      <pre>{shownOutput}</pre>
    {:else if item.status === 'running'}
      <div class="muted">Fetching…</div>
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
  .sources {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 4px 0;
  }
  .source {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 4px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--accent);
    font-size: 11.5px;
    text-align: left;
    min-width: 0;
    width: fit-content;
    max-width: 100%;
  }
  .source:hover {
    background: var(--surface-2);
    text-decoration: underline;
  }
  .u {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
