<script lang="ts">
  import type { AgentInfo } from '../../types';
  import { Network } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import { firstString, formatDuration, preview, resultDuration, type ToolItem } from './tool-utils';

  interface Props {
    item: ToolItem;
    agents: AgentInfo[];
    onShowAgents?: () => void;
  }

  let { item, agents, onShowAgents }: Props = $props();

  const linked = $derived(agents.filter((agent) => agent.parentToolCallId === item.toolCallId));
  const task = $derived(
    firstString(item.args, ['task', 'prompt', 'description', 'title']) ?? item.intent ?? '',
  );
  const duration = $derived(resultDuration(item.result));
  const meta = $derived(
    [
      linked.length > 0 ? `${linked.length} agent${linked.length === 1 ? '' : 's'}` : undefined,
      duration !== undefined ? formatDuration(duration) : undefined,
    ]
      .filter(Boolean)
      .join(' · '),
  );

  const STATUS_GLYPH: Record<AgentInfo['status'], string> = {
    pending: '◌',
    running: '●',
    waiting: '◉',
    completed: '✓',
    failed: '!',
    aborted: '■',
    parked: '◌',
  };

  const STATUS_LABEL: Record<AgentInfo['status'], string> = {
    pending: 'Pending',
    running: 'Working',
    waiting: 'Waiting',
    completed: 'Finished',
    failed: 'Failed',
    aborted: 'Stopped',
    parked: 'Parked',
  };
</script>

<ToolShell icon={Network} status={item.status} failed={item.status === 'failed' || item.result?.isError === true} meta={meta || undefined}>
  {#snippet summary()}
    <span class="line">Delegated{#if task}&nbsp;<span class="t">{preview(task, 70)}</span>{:else}&nbsp;work{/if}</span>
  {/snippet}
  {#snippet detail()}
    {#if task}
      <div class="row"><span class="k">task</span><span class="v">{task}</span></div>
    {/if}
    {#if linked.length > 0}
      <div class="agents">
        {#each linked as agent, linkIndex (linkIndex + ':' + agent.id)}
          <div class="agent">
            <span class="glyph s-{agent.status}" aria-hidden="true">{STATUS_GLYPH[agent.status]}</span>
            <span class="name">{agent.name}</span>
            {#if agent.task}<span class="role">{preview(agent.task, 50)}</span>{/if}
            <span class="st">{STATUS_LABEL[agent.status]}</span>
          </div>
        {/each}
      </div>
    {:else}
      <div class="muted">No linked agents reported yet</div>
    {/if}
    {#if onShowAgents}
      <button type="button" class="view" onclick={onShowAgents}>View agents</button>
    {/if}
  {/snippet}
</ToolShell>

<style>
  .line {
    min-width: 0;
  }
  .t {
    color: var(--text);
  }
  .agents {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin: 4px 0;
  }
  .agent {
    display: flex;
    align-items: baseline;
    gap: 7px;
    min-width: 0;
    font-size: 12px;
  }
  .glyph {
    flex: none;
    width: 12px;
    text-align: center;
  }
  .s-running {
    color: var(--accent);
  }
  .s-waiting {
    color: var(--warn);
  }
  .s-completed {
    color: var(--good);
  }
  .s-failed,
  .s-aborted {
    color: var(--bad);
  }
  .s-pending,
  .s-parked {
    color: var(--muted);
  }
  .name {
    flex: none;
    font-weight: 500;
  }
  .role {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .st {
    flex: none;
    color: var(--muted);
    font-size: 11px;
  }
  .view {
    margin-top: 4px;
    padding: 3px 10px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--accent);
    font-size: 11.5px;
  }
  .view:hover {
    background: var(--surface-3);
  }
</style>
