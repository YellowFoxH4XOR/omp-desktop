<script lang="ts">
  import { ClipboardList } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import Markdown from '../conversation/Markdown.svelte';
  import { planOutcome } from '../../plan';
  import { resultText, type ToolItem } from './tool-utils';

  interface Props {
    item: ToolItem;
  }

  let { item }: Props = $props();

  const plan = $derived(typeof item.args.plan === 'string' ? item.args.plan : '');
  const heading = $derived(plan.match(/^\s*#{1,3}\s+(.+)/)?.[1]?.trim() ?? 'Plan');
  const outcome = $derived(item.status === 'running' ? 'pending' : planOutcome(resultText(item.result)));
  const LABEL = { approved: 'Approved · ran in Auto', revising: 'Revising with feedback', declined: 'Declined', pending: 'Awaiting review' };
</script>

<ToolShell icon={ClipboardList} status={item.status} failed={item.status === 'failed' || item.result?.isError === true}>
  {#snippet summary()}
    <span class="line">{heading}<span class={`outcome ${outcome}`}>{LABEL[outcome]}</span></span>
  {/snippet}
  {#snippet detail()}
    {#if plan}<div class="plan"><Markdown text={plan} /></div>{/if}
  {/snippet}
</ToolShell>

<style>
  .line {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .outcome {
    padding: 1px 7px;
    border-radius: 999px;
    font-size: 11px;
    font-weight: 600;
    background: var(--surface-2);
    color: var(--muted);
  }
  .outcome.approved { background: var(--good-bg); color: var(--good); }
  .outcome.revising, .outcome.pending { background: var(--warn-bg); color: var(--warn); }
  .outcome.declined { background: var(--bad-bg); color: var(--bad); }
  .plan {
    padding: 4px 2px;
    font-size: 13px;
  }
</style>
