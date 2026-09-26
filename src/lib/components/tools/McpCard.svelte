<script lang="ts">
  import { Code2, Plug } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import CodeBlock from '../conversation/CodeBlock.svelte';
  import { formatDuration, prettyJson, resultDuration, resultText, type ToolItem } from './tool-utils';
  import { argPills, formatResult, humanize, resultSummary, type McpCall } from './mcp-tools.svelte';

  let { item, call }: { item: ToolItem; call: McpCall } = $props();

  const failed = $derived(item.status === 'failed' || item.result?.isError === true);
  const running = $derived(item.status === 'running');
  const pills = $derived(call.kind === 'call' ? argPills(call.args) : []);
  const outcome = $derived(running ? undefined : failed ? 'failed' : call.kind === 'call' || call.kind === 'script' ? resultSummary(item) : undefined);
  const duration = $derived(resultDuration(item.result));
  const meta = $derived([outcome, duration !== undefined ? formatDuration(duration) : undefined].filter(Boolean).join(' · ') || undefined);
  const output = $derived(resultText(item.result ?? item.partial));
  const shownOutput = $derived(output.length > 6000 ? `${formatResult(output).slice(0, 6000)}\n…` : formatResult(output));

  const verb = $derived.by(() => {
    switch (call.kind) {
      case 'script': return running ? 'Running MCP script' : 'Ran MCP script';
      case 'search': return running ? 'Searching MCP tools' : 'Searched MCP tools';
      case 'describe': return 'Looked up';
      case 'connect': return running ? 'Connecting to' : 'Connected to';
      case 'list': return 'Listed tools on';
      case 'status': return 'Checked MCP servers';
      case 'instructions': return 'Read instructions for';
      case 'auth': return 'Signing in to';
      case 'install': return 'Adding MCP server';
      case 'other': return 'MCP';
      default: return '';
    }
  });
  const scriptPreview = $derived(call.code?.split('\n').map(line => line.trim()).find(line => line && !line.startsWith('//')) ?? '');
</script>

<ToolShell icon={call.kind === 'script' ? Code2 : Plug} status={item.status} {failed} {meta}>
  {#snippet summary()}
    <span class="line">
      {#if call.kind === 'call'}
        {#if call.server}<span class="server">{call.server}</span>{/if}
        <span class="tool-name">{humanize(call.tool ?? item.toolName)}</span>
        {#each pills as [key, value] (key)}<span class="pill"><span class="pk">{key}</span> {value}</span>{/each}
      {:else}
        <span class="tool-name">{verb}</span>
        {#if call.kind === 'search' && call.query}<span class="q">“{call.query}”</span>
        {:else if call.kind === 'describe' && call.tool}{#if call.server}<span class="server">{call.server}</span>{/if}<span class="q">{humanize(call.tool)}</span>
        {:else if call.kind === 'script' && scriptPreview}<code class="script">{scriptPreview}</code>
        {:else if call.kind === 'other' && call.tool}<span class="q">{call.tool}</span>
        {:else if call.server}<span class="server">{call.server}</span>{/if}
      {/if}
    </span>
  {/snippet}
  {#snippet detail()}
    {#if call.kind === 'script' && call.code}
      <div class="section">Script</div>
      <CodeBlock code={call.code} lang="javascript" />
    {:else if call.args && Object.keys(call.args).length}
      <div class="section">Arguments</div>
      <div class="args">
        {#each Object.entries(call.args) as [key, value] (key)}
          <span class="k">{key}</span><code class="v">{typeof value === 'object' ? prettyJson(value, 400) : String(value)}</code>
        {/each}
      </div>
    {/if}
    {#if output}
      <div class="section">{failed ? 'Error' : 'Result'}</div>
      <pre class:err={failed}>{shownOutput}</pre>
    {:else if running}
      <p class="muted">Waiting for the server…</p>
    {/if}
  {/snippet}
</ToolShell>

<style>
  .line {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    min-width: 0;
    max-width: 100%;
    overflow: hidden;
  }
  .server {
    flex: none;
    display: inline-flex;
    align-items: center;
    height: 19px;
    padding: 0 7px;
    border-radius: 999px;
    background: var(--accent-bg);
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.01em;
  }
  .tool-name {
    flex: none;
    color: var(--text);
    font-weight: 500;
  }
  .q {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
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
  .script {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted) !important;
  }
  .section {
    margin: 2px 0 6px;
    color: var(--subtle);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
  }
  .section:not(:first-child) {
    margin-top: 12px;
  }
  .args {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: 4px 12px;
    align-items: baseline;
  }
  .k {
    color: var(--muted);
    font-size: 12px;
  }
  .v {
    min-width: 0;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    color: var(--text);
    font: 11.5px/1.5 var(--mono);
  }
  pre.err {
    color: var(--bad);
  }
  .muted {
    margin: 0;
    color: var(--muted);
  }
</style>
