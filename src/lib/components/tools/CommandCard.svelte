<script lang="ts">
  import ToolShell from './ToolShell.svelte';
  import {
    argCommand,
    exitCode,
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

  const command = $derived(argCommand(item.args) ?? item.intent ?? item.toolName);
  const output = $derived(resultText(item.result ?? item.partial));
  const code = $derived(exitCode(item.result));
  const duration = $derived(resultDuration(item.result ?? item.partial));
  const failed = $derived(item.status === 'failed' || item.result?.isError === true || (code !== undefined && code !== 0));
  const meta = $derived(
    [
      code !== undefined ? `exit ${code}` : undefined,
      duration !== undefined ? formatDuration(duration) : undefined,
    ]
      .filter(Boolean)
      .join(' · '),
  );
  const truncated = $derived(output.length > 4000);
  const shownOutput = $derived(truncated ? output.slice(-4000) : output);
</script>

<ToolShell status={item.status} {failed} meta={meta || undefined}>
  {#snippet summary()}
    <span class="line">Ran <code class="c">{preview(command, 90)}</code></span>
  {/snippet}
  {#snippet detail()}
    <div class="row"><span class="k">$</span><span class="v cmd">{command}</span></div>
    {#if item.intent}
      <div class="row"><span class="k">intent</span><span class="v muted">{item.intent}</span></div>
    {/if}
    {#if shownOutput}
      <pre class:err={failed}>{#if truncated}<span class="muted">… truncated …&#10;</span>{/if}{shownOutput}</pre>
    {:else if item.status === 'running'}
      <div class="muted">Running…</div>
    {:else}
      <div class="muted">No output</div>
    {/if}
    {#if item.result?.isError}
      <div class="err">Command reported an error</div>
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
    color: var(--text);
  }
</style>
