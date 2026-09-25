<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Check, Copy } from '@lucide/svelte';
  import { highlightCode, plainCodeHtml } from './highlight';

  interface Props {
    code: string;
    lang?: string;
    /** False while the fence is still streaming open; highlight only when true. */
    complete?: boolean;
  }

  let { code, lang, complete = true }: Props = $props();

  let html = $state<string | null>(null);
  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => {
    if (!complete) {
      html = null;
      return;
    }
    const source = code;
    const language = lang;
    html = null;
    let stale = false;
    void highlightCode(source, language).then((result) => {
      if (!stale) html = result;
    });
    return () => {
      stale = true;
    };
  });

  async function copy() {
    try {
      await navigator.clipboard.writeText(code);
      copied = true;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = false), 1500);
    } catch {
      // Clipboard unavailable; leave the button idle.
    }
  }

  onDestroy(() => clearTimeout(copyTimer));
</script>

<div class="code-block">
  <div class="code-head">
    <span class="lang">{lang || 'code'}</span>
    <button type="button" class="copy" onclick={copy} aria-label="Copy code">
      {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
    </button>
  </div>
  {#if html}
    <div class="shiki-wrap">{@html html}</div>
  {:else if !complete}
    <!-- Keep an open fence as text while it streams; replacing a growing HTML
         string on every token defeats incremental rendering and can flicker. -->
    <div class="shiki-wrap"><pre class="shiki-plain"><code>{code}</code></pre></div>
  {:else}
    <div class="shiki-wrap">{@html plainCodeHtml(code)}</div>
  {/if}
</div>

<style>
  .code-block {
    position: relative;
    margin: 10px 0;
    border: 1px solid var(--line);
    border-radius: var(--radius-lg);
    background: var(--surface);
    overflow: hidden;
  }
  .code-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 30px;
    padding: 0 6px 0 12px;
    border-bottom: 1px solid var(--line);
  }
  .lang {
    font-size: 11px;
    font-weight: 500;
    color: var(--subtle);
  }
  .copy {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--subtle);
    opacity: 0;
    transition: opacity 0.12s;
  }
  .code-block:hover .copy,
  .copy:focus-visible {
    opacity: 1;
  }
  .copy:hover {
    color: var(--text);
    background: var(--surface-2);
  }
  .shiki-wrap {
    overflow-x: auto;
    font-size: 12.5px;
    line-height: 1.6;
  }
  .shiki-wrap :global(pre) {
    margin: 0;
    padding: 10px 14px 12px;
    background: transparent !important;
  }
  .shiki-wrap :global(code) {
    font-family: var(--mono);
    padding: 0;
    background: transparent;
    border-radius: 0;
    font-size: inherit;
  }
  /* Dual-theme shiki output: pick the variable matching the app theme. Only
     foreground styling; token spans must never paint their own background. */
  .shiki-wrap :global(.shiki),
  .shiki-wrap :global(.shiki span) {
    color: var(--shiki-dark);
    font-style: var(--shiki-dark-font-style);
    font-weight: var(--shiki-dark-font-weight);
    text-decoration: var(--shiki-dark-text-decoration);
  }
  @media (prefers-color-scheme: light) {
    :global(:root:not([data-theme='dark'])) .shiki-wrap :global(.shiki),
    :global(:root:not([data-theme='dark'])) .shiki-wrap :global(.shiki span) {
      color: var(--shiki-light);
      font-style: var(--shiki-light-font-style);
      font-weight: var(--shiki-light-font-weight);
      text-decoration: var(--shiki-light-text-decoration);
    }
  }
  :global(:root[data-theme='light']) .shiki-wrap :global(.shiki),
  :global(:root[data-theme='light']) .shiki-wrap :global(.shiki span) {
    color: var(--shiki-light);
    font-style: var(--shiki-light-font-style);
    font-weight: var(--shiki-light-font-weight);
    text-decoration: var(--shiki-light-text-decoration);
  }
</style>
