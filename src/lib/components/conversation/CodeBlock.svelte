<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Check, Copy } from '@lucide/svelte';
  import { highlightCode, plainCodeHtml } from './highlight';
  import { sanitizeHtml } from './markdown';

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
    let stale = false;
    void highlightCode(source, language).then((result) => {
      if (!stale) html = result ? sanitizeHtml(result) : null;
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
  {:else}
    <div class="shiki-wrap">{@html plainCodeHtml(code)}</div>
  {/if}
</div>

<style>
  .code-block {
    margin: 6px 0;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    overflow: hidden;
  }
  .code-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 3px 6px 3px 10px;
    border-bottom: 1px solid var(--line);
    background: var(--surface-2);
  }
  .lang {
    font-size: 10px;
    color: var(--muted);
    text-transform: lowercase;
    letter-spacing: 0.04em;
  }
  .copy {
    display: inline-flex;
    align-items: center;
    padding: 3px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--muted);
  }
  .copy:hover {
    color: var(--text);
    background: var(--surface-3);
  }
  .shiki-wrap {
    overflow-x: auto;
    font-size: 12px;
    line-height: 1.5;
  }
  .shiki-wrap :global(pre) {
    margin: 0;
    padding: 8px 10px;
    background: transparent !important;
  }
  .shiki-wrap :global(code) {
    font-family: var(--mono);
  }
  /* Dual-theme shiki output: pick the variable matching the app theme. */
  .shiki-wrap :global(.shiki),
  .shiki-wrap :global(.shiki span) {
    color: var(--shiki-dark);
    background-color: var(--shiki-dark-bg);
    font-style: var(--shiki-dark-font-style);
    font-weight: var(--shiki-dark-font-weight);
    text-decoration: var(--shiki-dark-text-decoration);
  }
  @media (prefers-color-scheme: light) {
    :global(:root:not([data-theme='dark'])) .shiki-wrap :global(.shiki),
    :global(:root:not([data-theme='dark'])) .shiki-wrap :global(.shiki span) {
      color: var(--shiki-light);
      background-color: var(--shiki-light-bg);
      font-style: var(--shiki-light-font-style);
      font-weight: var(--shiki-light-font-weight);
      text-decoration: var(--shiki-light-text-decoration);
    }
  }
  :global(:root[data-theme='light']) .shiki-wrap :global(.shiki),
  :global(:root[data-theme='light']) .shiki-wrap :global(.shiki span) {
    color: var(--shiki-light);
    background-color: var(--shiki-light-bg);
    font-style: var(--shiki-light-font-style);
    font-weight: var(--shiki-light-font-weight);
    text-decoration: var(--shiki-light-text-decoration);
  }
</style>
