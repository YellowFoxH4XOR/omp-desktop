<script lang="ts">
  import { BRAND_ICONS, type BrandIconName } from '../../brand-icons';

  /** A vendored brand logo; single-color logos follow the text color. */
  let { name, size = 16, label }: { name: BrandIconName; size?: number; label?: string } = $props();
</script>

<span class="brand" style={`--size:${size}px`} role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : 'true'}>{@html BRAND_ICONS[name]}</span>

<style>
  .brand {
    flex: none;
    display: inline-flex;
    width: var(--size);
    height: var(--size);
    color: var(--text);
  }
  .brand :global(svg) {
    width: 100%;
    height: 100%;
  }
</style>
