<script lang="ts">
  import { onMount } from 'svelte';
  import { VList } from 'virtua/svelte';
  import { openPath } from '@tauri-apps/plugin-opener';
  import {
    ChevronDown,
    ChevronUp,
    Columns2,
    Copy,
    ExternalLink,
    FileDiff,
    FolderGit2,
    List,
    LoaderCircle,
    Maximize2,
    Minimize2,
    RefreshCcw,
    Undo2,
    X,
  } from '@lucide/svelte';
  import { api, onBackendEvent } from '$lib/api';
  import type { ChangedFile, ChangesSummary, GitFile, Thread } from '$lib/types';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import DiffEditor from './DiffEditor.svelte';
  import { detectEol, errorMessage, joinPath, statusMeta, type EolKind } from './util';

  const MODE_KEY = 'omp.changes.diffMode';
  const NARROW_PX = 460;

  let {
    thread,
    onClose,
    focusPath,
  }: {
    thread: Thread;
    onClose: () => void;
    focusPath?: string;
  } = $props();

  type ConfirmState = {
    title: string;
    path: string;
    detail?: string;
    confirmLabel: string;
    run: () => Promise<void>;
  };

  let panelEl: HTMLElement | undefined = $state();
  let diffEditor: ReturnType<typeof DiffEditor> | undefined = $state();

  let summary = $state<ChangesSummary | null>(null);
  let loading = $state(true);
  let refreshing = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  let selectedPath = $state<string | null>(null);
  let gitFile = $state<GitFile | null>(null);
  let fileLoading = $state(false);
  let fileError = $state<string | null>(null);
  let fileEol = $state<{ orig: EolKind; cur: EolKind }>({ orig: 'none', cur: 'none' });

  let mode = $state<'unified' | 'split'>('unified');
  let narrow = $state(false);
  let expanded = $state(false);
  let confirm = $state<ConfirmState | null>(null);
  let confirmBusy = $state(false);
  let copied = $state<'path' | 'selection' | null>(null);

  let fileSeq = 0;
  let refreshTimer: ReturnType<typeof setTimeout> | undefined;
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;
  let lastFocus: string | undefined;

  const worktreeRoot = $derived(thread.worktreePath ?? thread.cwd);
  const selectedFile = $derived(summary?.files.find((f) => f.path === selectedPath) ?? null);
  const diffable = $derived(!!gitFile && !gitFile.binary && !gitFile.tooLarge);
  // Hunk reverts splice HEAD text into the worktree file. CodeMirror stores
  // docs LF-normalized, so splicing is only byte-exact for LF files; anything
  // else (CRLF/mixed) gets whole-file revert only.
  const hunkRevertable = $derived(
    diffable &&
      !confirmBusy &&
      fileEol.cur !== 'mixed' &&
      fileEol.orig !== 'mixed' &&
      fileEol.orig !== 'crlf',
  );

  function setMode(next: 'unified' | 'split') {
    mode = next;
    try {
      localStorage.setItem(MODE_KEY, next);
    } catch {
      // localStorage may be unavailable; mode persistence is best-effort.
    }
  }

  function showNotice(text: string) {
    notice = text;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = null), 5000);
  }

  function showCopied(which: 'path' | 'selection') {
    copied = which;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied = null), 1500);
  }

  async function copyText(text: string): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      try {
        const ta = document.createElement('textarea');
        ta.value = text;
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        const ok = document.execCommand('copy');
        ta.remove();
        return ok;
      } catch {
        return false;
      }
    }
  }

  async function copyPath() {
    if (!selectedPath) return;
    if (await copyText(selectedPath)) showCopied('path');
  }

  async function copySelection() {
    const text = diffEditor?.selectionText() ?? '';
    if (!text) {
      showNotice('No selection in the diff editor.');
      return;
    }
    if (await copyText(text)) showCopied('selection');
  }

  async function openExternally() {
    if (!selectedPath) return;
    try {
      await openPath(joinPath(worktreeRoot, selectedPath));
    } catch (err) {
      showNotice(`Could not open file: ${errorMessage(err)}`);
    }
  }

  async function loadFile(path: string) {
    const seq = ++fileSeq;
    fileLoading = true;
    fileError = null;
    try {
      const f = await api.gitFile(thread.id, path);
      if (seq !== fileSeq || selectedPath !== path) return;
      gitFile = f;
      fileEol = { orig: detectEol(f.old), cur: detectEol(f.current) };
    } catch (err) {
      if (seq !== fileSeq || selectedPath !== path) return;
      gitFile = null;
      fileError = errorMessage(err);
    } finally {
      if (seq === fileSeq) fileLoading = false;
    }
  }

  function selectFile(path: string) {
    if (narrow) expanded = true;
    if (selectedPath === path) return;
    selectedPath = path;
    gitFile = null;
    fileError = null;
    void loadFile(path);
  }

  /** Reload the changed-file list and the open file. Exported as a refresh hook. */
  export async function refresh() {
    refreshing = true;
    try {
      const s = await api.gitStatus(thread.id);
      summary = s;
      error = null;
      if (!s.isRepo) {
        selectedPath = null;
        gitFile = null;
        return;
      }
      if (selectedPath && !s.files.some((f) => f.path === selectedPath)) {
        selectedPath = null;
        gitFile = null;
      }
      if (!selectedPath) {
        const target =
          (focusPath && focusPath !== lastFocus && s.files.some((f) => f.path === focusPath)
            ? focusPath
            : undefined) ??
          s.files[0]?.path ??
          null;
        if (target) {
          selectedPath = target;
          void loadFile(target);
        }
      } else {
        void loadFile(selectedPath);
      }
      lastFocus = focusPath;
    } catch (err) {
      error = errorMessage(err);
    } finally {
      loading = false;
      refreshing = false;
    }
  }

  function scheduleRefresh() {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => void refresh(), 250);
  }

  function requestFileRevert() {
    if (!selectedPath || !selectedFile) return;
    const path = selectedPath;
    const meta = statusMeta(selectedFile.status);
    const untracked = meta.cls === 'added';
    confirm = {
      title: 'Revert file to HEAD?',
      path,
      detail: untracked
        ? 'This file is not tracked by git. Reverting deletes it from the working tree.'
        : `All uncommitted changes in this file are discarded (${meta.label}). This cannot be undone.`,
      confirmLabel: 'Revert file',
      run: async () => {
        await api.gitRevertFile(thread.id, path);
        showNotice(`Reverted ${path}`);
        await refresh();
      },
    };
  }

  function requestHunkRevert(revert: () => string, scope: string) {
    const file = gitFile;
    const path = selectedPath;
    if (!file || !path || !hunkRevertable) return;
    confirm = {
      title: 'Revert hunk to HEAD?',
      path,
      detail: `Restores ${scope} to the HEAD version.`,
      confirmLabel: 'Revert hunk',
      run: async () => {
        const content = revert();
        await api.gitWriteIfUnchanged(thread.id, path, file.currentHash, content);
        showNotice(`Reverted hunk in ${path}`);
        await refresh();
      },
    };
  }

  async function runConfirm() {
    const c = confirm;
    if (!c || confirmBusy) return;
    confirmBusy = true;
    try {
      await c.run();
      confirm = null;
    } catch (err) {
      confirm = null;
      showNotice(`Revert failed: ${errorMessage(err)}`);
      // The write may have raced a newer file version; resync from git.
      await refresh();
    } finally {
      confirmBusy = false;
    }
  }

  function onWindowKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && expanded && !confirm) {
      e.stopPropagation();
      expanded = false;
    }
  }

  // Re-focus when the parent points at a different file after mount.
  $effect(() => {
    if (focusPath && focusPath !== lastFocus && summary?.files.some((f) => f.path === focusPath)) {
      lastFocus = focusPath;
      selectFile(focusPath);
    }
  });

  onMount(() => {
    try {
      const saved = localStorage.getItem(MODE_KEY);
      if (saved === 'unified' || saved === 'split') mode = saved;
    } catch {
      // best-effort
    }
    void refresh();

    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? 0;
      narrow = w > 0 && w < NARROW_PX;
      if (!narrow) expanded = false;
    });
    if (panelEl) ro.observe(panelEl);

    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onBackendEvent((event) => {
      if (event.type === 'git_changed' && event.threadId === thread.id) scheduleRefresh();
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });

    return () => {
      disposed = true;
      unlisten?.();
      ro.disconnect();
      clearTimeout(refreshTimer);
      clearTimeout(noticeTimer);
      clearTimeout(copiedTimer);
    };
  });
</script>

<svelte:window onkeydown={onWindowKeydown} />

<aside class="changes-panel" bind:this={panelEl} aria-label="Changes">
  <header class="panel-head">
    <div class="panel-title">
      <FileDiff size={13} />
      <span>Changes</span>
      {#if summary?.branch}
        <span class="branch" title="Current branch">{summary.branch}</span>
      {/if}
    </div>
    <div class="panel-actions">
      {#if summary && summary.isRepo && summary.files.length > 0}
        <div class="mode-toggle" role="group" aria-label="Diff mode">
          <button
            type="button"
            class:active={mode === 'unified'}
            title="Unified diff"
            onclick={() => setMode('unified')}
          >
            <List size={12} />
          </button>
          <button
            type="button"
            class:active={mode === 'split'}
            title="Split diff"
            onclick={() => setMode('split')}
          >
            <Columns2 size={12} />
          </button>
        </div>
      {/if}
      <button
        type="button"
        class="icon-btn"
        title="Refresh changes"
        onclick={() => void refresh()}
        disabled={refreshing}
      >
        <RefreshCcw size={12} class={refreshing ? 'spin' : ''} />
      </button>
      <button type="button" class="icon-btn" title="Close changes" onclick={onClose}>
        <X size={12} />
      </button>
    </div>
  </header>

  {#if loading}
    <div class="state-msg">
      <LoaderCircle size={14} class="spin" />
      <span>Loading changes…</span>
    </div>
  {:else if error}
    <div class="state-msg">
      <span class="state-title">Could not load changes</span>
      <span class="state-detail">{error}</span>
      <button type="button" class="btn" onclick={() => void refresh()}>Retry</button>
    </div>
  {:else if summary && !summary.isRepo}
    <div class="state-msg">
      <FolderGit2 size={16} />
      <span class="state-title">Not a git repository</span>
      <span class="state-detail">Changes are tracked from git, so there is nothing to review here.</span>
    </div>
  {:else if summary && summary.files.length === 0}
    <div class="state-msg">
      <FileDiff size={16} />
      <span class="state-title">No changes</span>
      <span class="state-detail">The working tree matches HEAD.</span>
    </div>
  {:else if summary}
    <div class="panel-body" class:expanded class:narrow>
      <div class="file-pane" class:hidden={narrow && expanded}>
        <div class="files-summary">
          {summary.files.length} file{summary.files.length === 1 ? '' : 's'} changed
          <span class="totals">
            <span class="add">+{summary.additions}</span>
            <span class="del">−{summary.deletions}</span>
          </span>
        </div>
        <div class="file-list">
          <VList data={summary.files} getKey={(f: ChangedFile) => f.path} itemSize={28}>
            {#snippet children(f: ChangedFile)}
              {@const meta = statusMeta(f.status)}
              <button
                type="button"
                class="file-row"
                class:selected={f.path === selectedPath}
                onclick={() => selectFile(f.path)}
                title={f.path}
              >
                <span class="status status-{meta.cls}" title={meta.label}>{meta.code}</span>
                <span class="file-path">{f.path}</span>
                {#if f.binary}
                  <span class="binary-tag">bin</span>
                {:else}
                  <span class="file-stats">
                    {#if f.additions > 0}<span class="add">+{f.additions}</span>{/if}
                    {#if f.deletions > 0}<span class="del">−{f.deletions}</span>{/if}
                  </span>
                {/if}
              </button>
            {/snippet}
          </VList>
        </div>
      </div>

      <div class="diff-pane" class:overlay={narrow && expanded}>
        {#if selectedPath && selectedFile}
          <div class="diff-head">
            <div class="diff-path" title={selectedPath}>
              <span class="status status-{statusMeta(selectedFile.status).cls}">
                {statusMeta(selectedFile.status).code}
              </span>
              <span class="diff-path-text">{selectedPath}</span>
            </div>
            <div class="diff-actions">
              {#if narrow}
                <button
                  type="button"
                  class="icon-btn"
                  title={expanded ? 'Collapse review' : 'Expand review'}
                  onclick={() => (expanded = !expanded)}
                >
                  {#if expanded}<Minimize2 size={12} />{:else}<Maximize2 size={12} />{/if}
                </button>
              {/if}
              {#if diffable}
                <button type="button" class="icon-btn" title="Previous change" onclick={() => diffEditor?.prevChange()}>
                  <ChevronUp size={12} />
                </button>
                <button type="button" class="icon-btn" title="Next change" onclick={() => diffEditor?.nextChange()}>
                  <ChevronDown size={12} />
                </button>
              {/if}
              <button
                type="button"
                class="icon-btn"
                title={copied === 'selection' ? 'Copied' : 'Copy selection'}
                onclick={() => void copySelection()}
                disabled={!diffable}
              >
                <Copy size={12} />
              </button>
              <button
                type="button"
                class="icon-btn"
                title={copied === 'path' ? 'Copied' : 'Copy file path'}
                onclick={() => void copyPath()}
              >
                <span class="copy-path-label">{copied === 'path' ? '✓' : ''}</span>
                <FileDiff size={12} />
              </button>
              <button type="button" class="icon-btn" title="Open file externally" onclick={() => void openExternally()}>
                <ExternalLink size={12} />
              </button>
              <button
                type="button"
                class="icon-btn danger"
                title="Revert file to HEAD"
                onclick={requestFileRevert}
                disabled={confirmBusy}
              >
                <Undo2 size={12} />
              </button>
            </div>
          </div>

          {#if fileLoading && !gitFile}
            <div class="state-msg small"><LoaderCircle size={13} class="spin" /><span>Loading diff…</span></div>
          {:else if fileError}
            <div class="state-msg small">
              <span class="state-title">Could not load file</span>
              <span class="state-detail">{fileError}</span>
              <button type="button" class="btn" onclick={() => selectedPath && void loadFile(selectedPath)}>Retry</button>
            </div>
          {:else if gitFile && gitFile.binary}
            <div class="state-msg small">
              <span class="state-title">Binary file</span>
              <span class="state-detail">No text diff available.</span>
              <button type="button" class="btn" onclick={() => void openExternally()}>Open externally</button>
            </div>
          {:else if gitFile && gitFile.tooLarge}
            <div class="state-msg small">
              <span class="state-title">File too large to diff</span>
              <span class="state-detail">Open it in an external editor instead.</span>
              <button type="button" class="btn" onclick={() => void openExternally()}>Open externally</button>
            </div>
          {:else if gitFile}
            <DiffEditor
              bind:this={diffEditor}
              path={gitFile.path}
              original={gitFile.old}
              current={gitFile.current}
              {mode}
              canRevertHunk={hunkRevertable}
              onRequestRevertHunk={requestHunkRevert}
            />
            {#if !hunkRevertable && diffable && (fileEol.cur === 'mixed' || fileEol.orig === 'mixed' || fileEol.orig === 'crlf')}
              <div class="diff-note">Hunk revert disabled: mixed or CRLF line endings.</div>
            {/if}
          {/if}
        {:else}
          <div class="state-msg small"><span class="state-detail">Select a file to review its diff.</span></div>
        {/if}
      </div>
    </div>
  {/if}

  {#if notice}
    <div class="notice" role="status">{notice}</div>
  {/if}

  {#if confirm}
    <ConfirmDialog
      title={confirm.title}
      path={confirm.path}
      detail={confirm.detail}
      confirmLabel={confirm.confirmLabel}
      busy={confirmBusy}
      onConfirm={() => void runConfirm()}
      onCancel={() => (confirm = null)}
    />
  {/if}
</aside>

<style>
  .changes-panel {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    color: var(--text);
    font-size: 12px;
    overflow: hidden;
  }
  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .panel-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--muted);
    min-width: 0;
  }
  .branch {
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0;
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .panel-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
  }
  .mode-toggle {
    display: flex;
    border: 1px solid var(--line);
    border-radius: 5px;
    overflow: hidden;
    margin-right: 4px;
  }
  .mode-toggle button {
    display: flex;
    align-items: center;
    padding: 3px 7px;
    border: none;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }
  .mode-toggle button.active {
    background: var(--surface-2);
    color: var(--text);
  }
  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2px;
    padding: 4px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
  }
  .icon-btn:hover:not(:disabled) {
    background: var(--surface-2);
    color: var(--text);
  }
  .icon-btn.danger:hover:not(:disabled) {
    color: var(--bad);
  }
  .icon-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .copy-path-label {
    font-size: 10px;
    width: 8px;
    color: var(--good);
  }
  .panel-body {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  /* Narrow panel: file list and diff share the space; the diff opens as an
     expanded review overlay instead. */
  .panel-body.narrow .file-pane {
    width: auto;
    flex: 1;
    min-width: 0;
    border-right: none;
  }
  .panel-body.narrow:not(.expanded) .diff-pane {
    display: none;
  }
  .file-pane {
    display: flex;
    flex-direction: column;
    width: 220px;
    min-width: 160px;
    flex: none;
    border-right: 1px solid var(--line);
    min-height: 0;
  }
  .file-pane.hidden {
    display: none;
  }
  .files-summary {
    padding: 6px 10px;
    font-size: 11px;
    color: var(--muted);
    border-bottom: 1px solid var(--line);
    display: flex;
    justify-content: space-between;
    gap: 6px;
    flex: none;
  }
  .totals {
    display: flex;
    gap: 6px;
    white-space: nowrap;
  }
  .add { color: var(--good); }
  .del { color: var(--bad); }
  .file-list {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .file-row {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    height: 28px;
    padding: 0 8px;
    border: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 11px;
    cursor: pointer;
    text-align: left;
  }
  .file-row:hover {
    background: var(--surface);
  }
  .file-row.selected {
    background: var(--surface-2);
  }
  .status {
    flex: none;
    width: 14px;
    text-align: center;
    font-size: 10px;
    font-weight: 700;
    border-radius: 3px;
  }
  .status-added { color: var(--good); }
  .status-modified { color: var(--warn); }
  .status-deleted { color: var(--bad); }
  .status-renamed { color: var(--accent); }
  .status-conflict { color: var(--bad); }
  .status-other { color: var(--muted); }
  .file-path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
  }
  .file-stats {
    flex: none;
    display: flex;
    gap: 4px;
    font-size: 10px;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
  }
  .binary-tag {
    flex: none;
    font-size: 9px;
    text-transform: uppercase;
    color: var(--muted);
    border: 1px solid var(--line);
    border-radius: 3px;
    padding: 0 3px;
  }

  .diff-pane {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }
  .diff-pane.overlay {
    position: fixed;
    top: 12px;
    right: 12px;
    bottom: 12px;
    left: 12px;
    z-index: 50;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 8px;
    box-shadow: 0 16px 60px color-mix(in srgb, var(--bg) 75%, transparent);
  }
  .diff-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 5px 8px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .diff-path {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 11px;
  }
  .diff-path-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .diff-actions {
    display: flex;
    align-items: center;
    gap: 1px;
    flex: none;
  }
  .diff-note {
    flex: none;
    padding: 4px 10px;
    font-size: 10px;
    color: var(--muted);
    border-top: 1px solid var(--line);
  }

  .state-msg {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    color: var(--muted);
    padding: 24px;
    text-align: center;
  }
  .state-msg.small {
    padding: 16px;
  }
  .state-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }
  .state-detail {
    font-size: 11px;
    max-width: 320px;
    line-height: 1.5;
  }
  .btn {
    font: inherit;
    font-size: 11px;
    padding: 4px 12px;
    border-radius: 5px;
    border: 1px solid var(--line);
    background: var(--surface-2);
    color: var(--text);
    cursor: pointer;
  }
  .btn:hover {
    border-color: var(--muted);
  }
  .notice {
    position: absolute;
    left: 10px;
    right: 10px;
    bottom: 10px;
    padding: 7px 10px;
    font-size: 11px;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 6px;
    box-shadow: 0 6px 24px color-mix(in srgb, var(--bg) 60%, transparent);
    z-index: 40;
  }
  :global(.spin) {
    animation: cmp-spin 0.9s linear infinite;
  }
  @keyframes cmp-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
