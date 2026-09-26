<script lang="ts">
  /** A number that eases to each new value instead of jumping. */
  let { value, prefix = '' }: { value: number; prefix?: string } = $props();

  // svelte-ignore state_referenced_locally
  let shown = $state(value);
  let frame: number | undefined;

  $effect(() => {
    const target = value;
    const from = shown;
    if (frame !== undefined) cancelAnimationFrame(frame);
    if (from === target || matchMedia('(prefers-reduced-motion: reduce)').matches) { shown = target; return; }
    const start = performance.now();
    const step = (now: number) => {
      const k = Math.min(1, (now - start) / 600);
      shown = Math.round(from + (target - from) * (1 - (1 - k) ** 3));
      frame = k < 1 ? requestAnimationFrame(step) : undefined;
    };
    frame = requestAnimationFrame(step);
    return () => { if (frame !== undefined) cancelAnimationFrame(frame); };
  });
</script>

<span class="count">{prefix}{shown}</span>

<style>
  .count { font-variant-numeric: tabular-nums; }
</style>
