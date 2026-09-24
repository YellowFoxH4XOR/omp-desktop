<script lang="ts">
  import CodeBlock from './CodeBlock.svelte';
  import { handleMarkdownClick } from './links';
  import { markdownSegments, type MdSegment } from './markdown';

  interface Props {
    text: string;
    streaming?: boolean;
  }

  let { text, streaming = false }: Props = $props();

  let settled: MdSegment[] = [];
  let settledLength = 0;
  let inspectedLength = 0;
  let currentLine = '';
  let openFence: string | undefined;
  let openFenceStart = 0;
  let openFenceContentStart = 0;
  let openFenceLang: string | undefined;
  let latestBoundary = 0;
  let activeText = '';
  let activeSegments: MdSegment[] = [];
  let previousValue = '';
  let previouslyStreaming = false;
  const MAX_ACTIVE_CHARS = 16_384;

  // Track blank-line boundaries and fences using only newly appended text.
  // Complete blocks are parsed once; the active tail is capped while a long
  // paragraph or fence grows. Completion reparses the authoritative full text.
  function escapeHtml(value: string): string {
    return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  function activeMarkdownSegments(input: string): MdSegment[] {
    if (input.length <= MAX_ACTIVE_CHARS) return markdownSegments(input, true);
    const split = input.length - MAX_ACTIVE_CHARS;
    return [
      { kind: 'html', html: `<span>${escapeHtml(input.slice(0, split))}</span>` },
      ...markdownSegments(input.slice(split), true),
    ];
  }

  function inspectAppended(value: string, start: number) {
    for (let index = start; index < value.length; index += 1) {
      const character = value[index];
      if (character !== '\n') {
        currentLine += character;
        continue;
      }

      const trimmed = currentLine.trimStart();
      const marker = currentLine.length - trimmed.length <= 3
        ? /^(`{3,}|~{3,})/.exec(trimmed)?.[1]
        : undefined;
      if (marker) {
        if (!openFence) {
          openFence = marker;
          openFenceStart = index - currentLine.length;
          openFenceContentStart = index + 1;
          openFenceLang = trimmed.slice(marker.length).trim().split(/\s+/, 1)[0] || undefined;
        } else if (marker[0] === openFence[0] && marker.length >= openFence.length) {
          openFence = undefined;
          openFenceStart = 0;
          openFenceContentStart = 0;
          openFenceLang = undefined;
        }
      }
      if (!openFence && currentLine.trim() === '') latestBoundary = index + 1;
      currentLine = '';
    }
    inspectedLength = value.length;
  }

  function reset() {
    settled = [];
    settledLength = 0;
    inspectedLength = 0;
    currentLine = '';
    openFence = undefined;
    openFenceStart = 0;
    openFenceContentStart = 0;
    openFenceLang = undefined;
    latestBoundary = 0;
    activeText = '';
    activeSegments = [];
    previousValue = '';
    previouslyStreaming = false;
  }

  function renderSegments(value: string, active: boolean): MdSegment[] {
    if (!active) {
      reset();
      previousValue = value;
      return markdownSegments(value, false);
    }
    if (!previouslyStreaming || !value.startsWith(previousValue)) reset();
    previouslyStreaming = true;
    inspectAppended(value, inspectedLength);
    previousValue = value;

    if (latestBoundary > settledLength) {
      settled.push(...markdownSegments(value.slice(settledLength, latestBoundary), false));
      settledLength = latestBoundary;
    }

    const nextActiveText = value.slice(settledLength);
    if (nextActiveText !== activeText) {
      activeText = nextActiveText;
      if (openFence) {
        activeSegments = [
          ...activeMarkdownSegments(value.slice(settledLength, openFenceStart)),
          {
            kind: 'code',
            code: value.slice(openFenceContentStart),
            lang: openFenceLang,
            complete: false,
          },
        ];
      } else {
        activeSegments = activeMarkdownSegments(nextActiveText);
      }
    }
    return [...settled, ...activeSegments];
  }
  const segments = $derived(renderSegments(text, streaming));
</script>

<!-- Sanitized markdown: anchors are routed through the opener plugin. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="md" onclick={handleMarkdownClick}>
  {#each segments as segment, i (i)}
    {#if segment.kind === 'code'}
      <CodeBlock code={segment.code ?? ''} lang={segment.lang} complete={segment.complete ?? true} />
    {:else}
      {@html segment.html}
    {/if}
  {/each}
</div>

<style>
  .md {
    font-size: 13px;
    line-height: 1.55;
    overflow-wrap: break-word;
  }
  .md :global(p) {
    margin: 0 0 8px;
  }
  .md :global(p:last-child) {
    margin-bottom: 0;
  }
  .md :global(h1),
  .md :global(h2),
  .md :global(h3),
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) {
    margin: 14px 0 6px;
    font-weight: 600;
    line-height: 1.3;
  }
  .md :global(h1) {
    font-size: 17px;
  }
  .md :global(h2) {
    font-size: 15px;
  }
  .md :global(h3) {
    font-size: 14px;
  }
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) {
    font-size: 13px;
  }
  .md :global(ul),
  .md :global(ol) {
    margin: 4px 0 8px;
    padding-left: 20px;
  }
  .md :global(li) {
    margin: 2px 0;
  }
  .md :global(li > ul),
  .md :global(li > ol) {
    margin: 2px 0;
  }
  .md :global(blockquote) {
    margin: 6px 0;
    padding: 2px 10px;
    border-left: 2px solid var(--line);
    color: var(--muted);
  }
  .md :global(code) {
    font-family: var(--mono);
    font-size: 0.92em;
    padding: 1px 4px;
    border-radius: 4px;
    background: var(--surface-2);
  }
  .md :global(pre code) {
    padding: 0;
    background: transparent;
  }
  .md :global(a) {
    color: var(--accent);
    text-decoration: none;
  }
  .md :global(a:hover) {
    text-decoration: underline;
  }
  .md :global(table) {
    border-collapse: collapse;
    margin: 6px 0 10px;
    font-size: 12px;
  }
  .md :global(th),
  .md :global(td) {
    border: 1px solid var(--line);
    padding: 4px 8px;
    text-align: left;
  }
  .md :global(th) {
    background: var(--surface-2);
    font-weight: 600;
  }
  .md :global(hr) {
    border: 0;
    border-top: 1px solid var(--line);
    margin: 12px 0;
  }
  .md :global(img) {
    max-width: 100%;
    border-radius: var(--radius);
  }
</style>
