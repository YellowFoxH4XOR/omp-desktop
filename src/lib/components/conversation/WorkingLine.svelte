<script lang="ts">
  import type { ConversationItem } from '../../types';
  import { argCommand, argPath, argQuery, argUrl, preview } from '../tools/tool-utils';

  /** A quiet, rotating line under the transcript saying what the agent is doing. */
  let { items }: { items: ConversationItem[] } = $props();

  const GENERIC = [
    'Connecting the pieces…',
    'Checking the details…',
    'Double-checking assumptions…',
    'Keeping the change focused…',
    'Following the thread…',
  ];

  function base(path: string): string {
    return path.split('/').filter(Boolean).at(-1) ?? path;
  }

  /** What the newest unfinished step is doing, in plain words. */
  const current = $derived.by((): string | null => {
    const last = items.at(-1);
    if (!last) return 'Getting started…';
    if (last.kind === 'text' && last.streaming) return null; // The words themselves show progress.
    if (last.kind === 'thinking') return 'Thinking it through…';
    if (last.kind !== 'tool' || last.status !== 'running') return 'Deciding the next step…';
    const name = last.toolName.toLowerCase();
    const path = argPath(last.args);
    if (/bash|shell|exec|command|terminal/.test(name)) {
      const command = argCommand(last.args);
      return command ? `Running ${preview(command, 48)}…` : 'Running a command…';
    }
    if (/edit|patch|replace/.test(name)) return path ? `Editing ${base(path)}…` : 'Editing…';
    if (/write|create/.test(name)) return path ? `Writing ${base(path)}…` : 'Writing a file…';
    if (/grep|search|find|glob/.test(name)) {
      const query = argQuery(last.args);
      return query ? `Searching for “${preview(query, 36)}”…` : 'Searching…';
    }
    if (name === 'ls' || name.includes('list')) return path ? `Looking through ${base(path)}…` : 'Looking around…';
    if (name.includes('read') || name === 'view') return path ? `Reading ${base(path)}…` : 'Reading…';
    if (/web|fetch|url|browse/.test(name)) {
      const url = argUrl(last.args);
      return url ? `Reading ${preview(url.replace(/^https?:\/\//, ''), 40)}…` : 'Looking it up…';
    }
    if (name === 'request_auto') return 'Waiting for your approval…';
    return `Using ${last.toolName}…`;
  });

  // Lead with the concrete step, then alternate with a generic line while it runs.
  let beat = $state(0);
  $effect(() => {
    current;
    beat = 0;
    const timer = setInterval(() => (beat += 1), 2400);
    return () => clearInterval(timer);
  });
  const line = $derived(current === null ? null : beat % 2 === 0 ? current : GENERIC[(beat >> 1) % GENERIC.length]);
</script>

{#if line}
  <div class="working" aria-hidden="true">
    <span class="orb"></span>
    {#key line}<span class="text">{line}</span>{/key}
  </div>
{/if}

<style>
  .working {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    max-width: calc(var(--content-width) + 48px);
    height: 26px;
    margin: 0 auto;
    padding: 0 28px;
    overflow: hidden;
    color: var(--subtle);
    font-size: 12.5px;
  }
  .orb {
    flex: none;
    width: 8px;
    height: 8px;
    margin-left: 6px;
    border-radius: 50%;
    background: var(--accent);
    animation: orb 1.6s ease-in-out infinite;
  }
  @keyframes orb {
    50% {
      transform: scale(0.6);
      opacity: 0.45;
    }
  }
  .text {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    animation: line-in 0.4s var(--ease) both;
  }
  @keyframes line-in {
    from {
      opacity: 0;
      transform: translateY(8px);
      filter: blur(2px);
    }
  }
</style>
