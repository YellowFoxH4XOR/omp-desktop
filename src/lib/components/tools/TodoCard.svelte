<script lang="ts">
  import { Check, Circle, LoaderCircle, ListTodo } from '@lucide/svelte';
  import ToolShell from './ToolShell.svelte';
  import { resultText, type ToolItem } from './tool-utils';

  interface Props {
    item: ToolItem;
  }

  interface TodoEntry {
    text: string;
    state: 'done' | 'active' | 'pending';
  }

  let { item }: Props = $props();

  function parseEntries(): TodoEntry[] {
    const details = item.result?.details ?? item.args['todos'] ?? item.args['items'];
    const list = Array.isArray(details)
      ? details
      : Array.isArray((details as Record<string, unknown> | undefined)?.['todos'])
        ? ((details as Record<string, unknown>)['todos'] as unknown[])
        : undefined;
    if (list) {
      return list
        .map((entry): TodoEntry | undefined => {
          if (typeof entry === 'string') return { text: entry, state: 'pending' };
          if (!entry || typeof entry !== 'object') return undefined;
          const record = entry as Record<string, unknown>;
          const text =
            typeof record['content'] === 'string'
              ? record['content']
              : typeof record['text'] === 'string'
                ? record['text']
                : typeof record['task'] === 'string'
                  ? record['task']
                  : undefined;
          if (!text) return undefined;
          const status = String(record['status'] ?? record['state'] ?? '').toLowerCase();
          const state =
            status === 'completed' || status === 'done'
              ? 'done'
              : status === 'in_progress' || status === 'active' || status === 'running'
                ? 'active'
                : 'pending';
          return { text, state };
        })
        .filter((entry): entry is TodoEntry => entry !== undefined);
    }
    // Fallback: parse markdown-ish checklist text.
    const text = resultText(item.result) || String(item.args['todos'] ?? '');
    return text
      .split('\n')
      .map((line) => line.trim())
      .filter((line) => line.length > 0)
      .map((line) => {
        const done = /^\[[x✓]\]|^[-*•]\s*\[[x✓]\]|^✓/.test(line);
        const active = /^[-*•]?\s*\[~\]|^●|^▶/.test(line);
        return {
          text: line.replace(/^[-*•]\s*/, '').replace(/^\[[ x✓~]\]\s*/, ''),
          state: done ? 'done' : active ? 'active' : 'pending',
        } as TodoEntry;
      });
  }

  const entries = $derived(parseEntries());
  const doneCount = $derived(entries.filter((entry) => entry.state === 'done').length);
  const meta = $derived(entries.length > 0 ? `${doneCount}/${entries.length} done` : undefined);
  const current = $derived(entries.find((entry) => entry.state === 'active'));
</script>

<ToolShell icon={ListTodo} status={item.status} failed={item.status === 'failed'} {meta}>
  {#snippet summary()}
    <span class="line">Plan{#if current}&nbsp;<span class="now">{current.text}</span>{/if}</span>
  {/snippet}
  {#snippet detail()}
    {#if entries.length > 0}
      <ul class="todos">
        {#each entries as entry, i (i)}
          <li class="todo s-{entry.state}">
            <span class="box" aria-hidden="true">
              {#if entry.state === 'done'}
                <Check size={11} />
              {:else if entry.state === 'active'}
                <LoaderCircle size={11} />
              {:else}
                <Circle size={10} />
              {/if}
            </span>
            <span class="txt">{entry.text}</span>
          </li>
        {/each}
      </ul>
    {:else}
      <div class="muted">No items</div>
    {/if}
  {/snippet}
</ToolShell>

<style>
  .line {
    min-width: 0;
  }
  .now {
    color: var(--text);
  }
  .todos {
    list-style: none;
    margin: 2px 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .todo {
    display: flex;
    align-items: baseline;
    gap: 7px;
    font-size: 12px;
  }
  .box {
    flex: none;
    display: inline-flex;
    color: var(--muted);
  }
  .s-done .box {
    color: var(--good);
  }
  .s-done .txt {
    color: var(--muted);
    text-decoration: line-through;
  }
  .s-active .box {
    color: var(--accent);
  }
</style>
