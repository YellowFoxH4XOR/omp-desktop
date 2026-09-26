<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Check, Copy } from '@lucide/svelte';
  import { highlightCode, plainCodeHtml } from './highlight';
  import { isDarkTheme, MAX_MERMAID_CHARS, renderMermaid } from './mermaid';

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

  // Mermaid fences render as diagrams once the fence closes.
  const isMermaid = $derived(
    (lang ?? '').trim().toLowerCase() === 'mermaid' && code.length <= MAX_MERMAID_CHARS,
  );
  let view = $state<'diagram' | 'code'>('diagram');
  let diagram = $state<string | null>(null);
  let diagramError = $state<string | null>(null);
  let dark = $state(false);

  $effect(() => {
    if (!isMermaid) return;
    dark = isDarkTheme();
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const update = () => (dark = isDarkTheme());
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    media.addEventListener('change', update);
    return () => {
      observer.disconnect();
      media.removeEventListener('change', update);
    };
  });

  $effect(() => {
    if (!isMermaid || !complete) {
      diagram = null;
      diagramError = null;
      return;
    }
    const source = code;
    const theme = dark;
    let stale = false;
    diagramError = null;
    renderMermaid(source, theme).then(
      (svg) => {
        if (!stale) diagram = svg;
      },
      (error: unknown) => {
        if (stale) return;
        diagram = null;
        const message = error instanceof Error ? error.message : String(error);
        diagramError = message.split('\n')[0] || 'Invalid diagram syntax.';
      },
    );
    return () => {
      stale = true;
    };
  });

  // Box-drawing art only connects when rows touch; normal code keeps its airy leading.
  const boxArt = $derived(/[\u2500-\u257F]/.test(code));

  const showDiagram = $derived(isMermaid && complete && view === 'diagram' && !diagramError);
</script>

<div class="code-block" class:box-art={boxArt}>
  <div class="code-head">
    <span class="lang">{lang || 'code'}</span>
    {#if isMermaid && complete && !diagramError}
      <div class="view-toggle" role="group" aria-label="Mermaid view">
        <button type="button" class:on={view === 'diagram'} aria-pressed={view === 'diagram'} onclick={() => (view = 'diagram')}>Diagram</button>
        <button type="button" class:on={view === 'code'} aria-pressed={view === 'code'} onclick={() => (view = 'code')}>Code</button>
      </div>
    {/if}
    <button type="button" class="copy" onclick={copy} aria-label="Copy code">
      {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
    </button>
  </div>
  {#if showDiagram}
    <div class="diagram" role="img" aria-label="Mermaid diagram">
      {#if diagram}{@html diagram}{:else}<span class="diagram-loading">Rendering diagram…</span>{/if}
    </div>
  {:else if html}
    {#if diagramError}<div class="diagram-error">Couldn’t render diagram: {diagramError}</div>{/if}
    <div class="shiki-wrap">{@html html}</div>
  {:else if !complete}
    <!-- Keep an open fence as text while it streams; replacing a growing HTML
         string on every token defeats incremental rendering and can flicker. -->
    <div class="shiki-wrap"><pre class="shiki-plain"><code>{code}</code></pre></div>
  {:else}
    {#if diagramError}<div class="diagram-error">Couldn’t render diagram: {diagramError}</div>{/if}
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
    flex: 1;
    font-size: 11px;
    font-weight: 500;
    color: var(--subtle);
  }
  .view-toggle {
    display: flex;
    gap: 2px;
    padding: 2px;
    margin-right: 4px;
    border-radius: 7px;
    background: var(--surface-2);
  }
  .view-toggle button {
    height: 20px;
    padding: 0 8px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--muted);
    font-size: 11px;
    font-weight: 500;
  }
  .view-toggle button:hover {
    color: var(--text);
  }
  .view-toggle button.on {
    background: var(--elevated);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
  .diagram {
    display: flex;
    justify-content: center;
    padding: 16px;
    overflow-x: auto;
    background: var(--bg);
  }
  .diagram :global(svg) {
    max-width: 100%;
    height: auto;
  }
  .diagram-loading {
    padding: 24px 0;
    color: var(--subtle);
    font-size: 12px;
  }
  .diagram-error {
    padding: 8px 14px;
    border-bottom: 1px solid var(--line);
    color: var(--warn);
    background: var(--warn-bg);
    font-size: 12px;
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
  .box-art .shiki-wrap {
    line-height: 1.2;
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
