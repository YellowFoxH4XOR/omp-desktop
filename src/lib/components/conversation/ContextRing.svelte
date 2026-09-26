<script lang="ts">
  import type { ContextUsage } from '../../types';

  let { usage }: { usage?: ContextUsage | null } = $props();

  const percent = $derived(usage?.percent != null ? Math.max(0, Math.min(100, Math.round(usage.percent))) : null);
  const RING = 2 * Math.PI * 6;
</script>

{#if percent !== null}
  <span
    class="context"
    class:high={percent >= 80}
    role="meter"
    aria-label="Context used"
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={percent}
    title={`Context ${usage?.tokens?.toLocaleString() ?? '?'} / ${usage?.contextWindow.toLocaleString()} tokens`}
  >
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <circle cx="8" cy="8" r="6" class="ring-track" />
      <circle cx="8" cy="8" r="6" class="ring-fill" stroke-dasharray={`${(RING * percent) / 100} ${RING}`} />
    </svg>
    <span class="context-pct">{percent}%</span>
  </span>
{/if}

<style>
  .context {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 0 6px;
    color: var(--subtle);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .context svg {
    transform: rotate(-90deg);
  }
  .ring-track {
    fill: none;
    stroke: var(--surface-3);
    stroke-width: 2.2;
  }
  .ring-fill {
    fill: none;
    stroke: var(--accent);
    stroke-width: 2.2;
    stroke-linecap: round;
    transition: stroke-dasharray 0.3s var(--ease);
  }
  .context.high .ring-fill {
    stroke: var(--warn);
  }
</style>
