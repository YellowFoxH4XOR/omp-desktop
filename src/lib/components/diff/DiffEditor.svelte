<script lang="ts">
  import { Compartment, EditorState, type Extension } from '@codemirror/state';
  import { defaultKeymap, history } from '@codemirror/commands';
  import { drawSelection, EditorView, keymap, lineNumbers } from '@codemirror/view';
  import { syntaxHighlighting, type LanguageSupport } from '@codemirror/language';
  import {
    MergeView,
    getChunks,
    goToNextChunk,
    goToPreviousChunk,
    unifiedMergeView,
    type Chunk,
  } from '@codemirror/merge';
  import { diffHighlight, diffTheme, languageFor } from './util';

  /**
   * CodeMirror 6 merge surface. Renders `original` (HEAD) vs `current`
   * (worktree) in unified or split mode. Read-only; hunk reverts are routed
   * through `onRequestRevertHunk` so the parent can confirm and persist them
   * via git_write_if_unchanged.
   */
  let {
    path,
    original,
    current,
    mode,
    canRevertHunk,
    onRequestRevertHunk,
  }: {
    path: string;
    original: string;
    current: string;
    mode: 'unified' | 'split';
    canRevertHunk: boolean;
    /**
     * Called when the user clicks a hunk revert control. `revert` applies the
     * revert inside the editor and returns the resulting full document text;
     * the parent persists it (guarded by currentHash) and refreshes.
     */
    onRequestRevertHunk: (revert: () => string, scope: string) => void;
  } = $props();

  let hostEl: HTMLDivElement | undefined = $state();
  let view: EditorView | undefined;
  let mergeView: MergeView | undefined;
  let loadedSupport: LanguageSupport | null = null;
  let langPath = '';
  let buildSeq = 0;

  const langCompartment = new Compartment();

  function baseExtensions(eol: string | null): Extension[] {
    return [
      ...(eol ? [EditorState.lineSeparator.of(eol)] : []),
      lineNumbers(),
      history(),
      drawSelection(),
      EditorState.readOnly.of(true),
      EditorView.editable.of(false),
      keymap.of([...defaultKeymap]),
      langCompartment.of([]),
      syntaxHighlighting(diffHighlight),
      diffTheme,
    ];
  }

  function chunkScope(chunk: Chunk, aDoc: string, bDoc: string): string {
    const aLines = aDoc === '' ? 0 : aDoc.split('\n').length;
    const bLines = bDoc === '' ? 0 : bDoc.split('\n').length;
    if (chunk.fromB === chunk.toB) {
      const line = Math.min(bLines, bDoc.slice(0, chunk.fromB).split('\n').length);
      return `inserted lines after line ${line} of current`;
    }
    const startB = bDoc.slice(0, chunk.fromB).split('\n').length;
    const endB = Math.min(bLines, bDoc.slice(0, chunk.endB).split('\n').length);
    if (chunk.fromA === chunk.toA) {
      return `added line${endB === startB ? '' : 's'} ${startB}${endB === startB ? '' : `–${endB}`}`;
    }
    const startA = aDoc.slice(0, chunk.fromA).split('\n').length;
    const endA = Math.min(aLines, aDoc.slice(0, chunk.endA).split('\n').length);
    return `lines ${startB}–${endB} (was ${startA}–${endA} in HEAD)`;
  }

  function mergeControlButtons(type: 'reject' | 'accept', action: (e: MouseEvent) => void): HTMLElement {
    const btn = document.createElement('button');
    btn.type = 'button';
    if (type === 'accept') {
      // "Accept" only mutates the in-editor baseline — meaningless for a
      // git-backed review, so it stays hidden.
      btn.style.display = 'none';
      return btn;
    }
    btn.className = 'cmp-hunk-revert';
    btn.textContent = 'Revert hunk';
    btn.onmousedown = (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (!canRevertHunk || !view) return;
      const widget = (e.currentTarget as HTMLElement).closest('.cm-deletedChunk');
      if (!widget) return;
      const pos = view.posAtDOM(widget);
      const found = findChunkAt(pos);
      if (!found || !found.precise) return;
      const scope = chunkScope(found, original, view.state.sliceDoc(0));
      const ev = e;
      const v = view;
      onRequestRevertHunk(() => {
        action(ev);
        return v.state.sliceDoc(0);
      }, scope);
    };
    return btn;
  }

  function findChunkAt(pos: number): Chunk | undefined {
    const chunks = mergeView ? mergeView.chunks : view ? (getChunks(view.state)?.chunks ?? null) : null;
    return chunks?.find((ch) => ch.fromB <= pos && ch.endB >= pos);
  }

  function renderSplitRevertButton(): HTMLElement {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'cmp-split-revert';
    btn.setAttribute('aria-label', 'Revert this hunk to HEAD');
    btn.title = 'Revert this hunk to HEAD';
    btn.textContent = '⇝';
    btn.onmousedown = (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (!canRevertHunk || !mergeView) return;
      const idx = Number((e.currentTarget as HTMLElement).dataset.chunk);
      const chunk = mergeView.chunks[idx];
      if (!chunk || !chunk.precise) return;
      const mv = mergeView;
      const scope = chunkScope(chunk, mv.a.state.sliceDoc(0), mv.b.state.sliceDoc(0));
      onRequestRevertHunk(() => {
        // Mirror MergeView.revertClicked: copy the chunk from A into B.
        const src = mv.a;
        const dest = mv.b;
        let insert = src.state.sliceDoc(chunk.fromA, Math.max(chunk.fromA, chunk.toA - 1));
        if (chunk.fromA !== chunk.toA && chunk.toB <= dest.state.doc.length) {
          insert += src.state.lineBreak;
        }
        dest.dispatch({
          changes: {
            from: chunk.fromB,
            to: Math.min(dest.state.doc.length, chunk.toB),
            insert,
          },
          userEvent: 'revert',
        });
        return dest.state.sliceDoc(0);
      }, scope);
    };
    return btn;
  }

  function build() {
    destroyViews();
    if (!hostEl) return;
    const eol = current.includes('\r\n') && !current.replace(/\r\n/g, '').includes('\n') ? '\r\n' : null;
    const ext = baseExtensions(eol);
    if (mode === 'split') {
      mergeView = new MergeView({
        a: { doc: original, extensions: ext },
        b: { doc: current, extensions: ext },
        parent: hostEl,
        orientation: 'a-b',
        highlightChanges: true,
        gutter: true,
        collapseUnchanged: { margin: 3, minSize: 4 },
        ...(canRevertHunk
          ? { revertControls: 'a-to-b' as const, renderRevertControl: renderSplitRevertButton }
          : {}),
      });
      view = mergeView.b;
    } else {
      view = new EditorView({
        state: EditorState.create({
          doc: current,
          extensions: [
            ...ext,
            unifiedMergeView({
              original,
              highlightChanges: true,
              gutter: true,
              syntaxHighlightDeletions: true,
              collapseUnchanged: { margin: 3, minSize: 4 },
              mergeControls: canRevertHunk ? mergeControlButtons : false,
            }),
          ],
        }),
        parent: hostEl,
      });
    }
    buildSeq++;
    void loadLanguage();
  }

  async function loadLanguage() {
    if (path !== langPath) {
      langPath = path;
      loadedSupport = null;
    }
    if (loadedSupport) return;
    const seq = buildSeq;
    const support = await languageFor(path);
    if (!support || seq !== buildSeq || !hostEl?.isConnected) return;
    loadedSupport = support;
    // Rebuild so unified deleted-chunk widgets (rendered lazily, capturing the
    // language facet at build time) pick up the grammar too.
    const scroller = activeView()?.scrollDOM;
    const top = scroller?.scrollTop ?? 0;
    const left = scroller?.scrollLeft ?? 0;
    build();
    const scroller2 = activeView()?.scrollDOM;
    if (scroller2) {
      scroller2.scrollTop = top;
      scroller2.scrollLeft = left;
    }
  }

  function activeView(): EditorView | undefined {
    return mergeView ? mergeView.b : view;
  }

  function destroyViews() {
    mergeView?.destroy();
    mergeView = undefined;
    view?.destroy();
    view = undefined;
  }

  $effect(() => {
    // Rebuild whenever the inputs change; cleanup destroys the previous view.
    void path;
    void original;
    void current;
    void mode;
    void canRevertHunk;
    if (hostEl) build();
    return destroyViews;
  });

  export function nextChange() {
    const v = activeView();
    if (v) goToNextChunk(v);
  }

  export function prevChange() {
    const v = activeView();
    if (v) goToPreviousChunk(v);
  }

  export function selectionText(): string {
    const v = activeView();
    if (!v) return '';
    const { from, to } = v.state.selection.main;
    return from === to ? '' : v.state.sliceDoc(from, to);
  }
</script>

<div class="diff-editor" bind:this={hostEl}></div>

<style>
  .diff-editor {
    flex: 1;
    min-height: 0;
    min-width: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }
  .diff-editor :global(.cm-editor) {
    height: 100%;
  }
  .diff-editor :global(.cm-mergeView) {
    height: 100%;
    overflow: auto;
  }
  .diff-editor :global(.cmp-split-revert) {
    color: var(--muted);
    font-size: 12px;
    line-height: 1.6;
    padding: 0;
  }
  .diff-editor :global(.cmp-split-revert:hover) {
    color: var(--bad);
  }
</style>
