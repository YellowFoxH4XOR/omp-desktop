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
  let previousLength = 0;
  let previouslyStreaming = false;

  // Commit blocks only at blank lines outside fences. The active tail is the
  // only markdown reparsed on a streaming update; final text parses once in
  // full so list/reference semantics are authoritative at completion.
  function stableBoundary(input: string): number {
    let fence: string | undefined;
    let length = 0;
    let boundary = 0;
    for (const line of input.split('\n')) {
      const trimmed = line.trimStart();
      const marker = line.length - trimmed.length <= 3 ? /^(`{3,}|~{3,})/.exec(trimmed)?.[1] : undefined;
      if (marker) {
        if (!fence) fence = marker;
        else if (marker[0] === fence[0] && marker.length >= fence.length) fence = undefined;
      }
      length += line.length + 1;
      if (!fence && line.trim() === '' && length <= input.length) boundary = length;
    }
    return boundary;
  }

  function renderSegments(value: string, active: boolean): MdSegment[] {
    if (!active) {
      settled = [];
      settledLength = 0;
      previousLength = value.length;
      previouslyStreaming = false;
      return markdownSegments(value, false);
    }
    if (!previouslyStreaming || value.length < previousLength || settledLength > value.length) {
      settled = [];
      settledLength = 0;
    }
    previouslyStreaming = true;
    const tail = value.slice(settledLength);
    const boundary = stableBoundary(tail);
    if (boundary > 0) {
      settled.push(...markdownSegments(tail.slice(0, boundary), false));
      settledLength += boundary;
    }
    previousLength = value.length;
    return [...settled, ...markdownSegments(value.slice(settledLength), true)];
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
