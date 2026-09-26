<script lang="ts">
  import { onMount } from 'svelte';
  import { VList } from 'virtua/svelte';
  import { api, onBackendEvent } from '$lib/api';
  import {
    ChevronDown,
    ChevronUp,
    Columns2,
    Copy,
    ExternalLink,
    FileDiff,
    FolderGit2,
    GitBranch,
    List,
    LoaderCircle,
    Maximize2,
    Minimize2,
    RefreshCcw,
    Undo2,
    X,
  } from '@lucide/svelte';
  import type { ChangedFile, ChangesSummary, GitFile, Thread } from '$lib/types';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import DiffEditor from './DiffEditor.svelte';
  import { detectEol, errorMessage, joinPath, statusMeta, type EolKind } from './util';

  const MODE_KEY = 'pidesk.changes.diffMode';
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
  let refreshSeq = 0;
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
      fileEol.cur !== 'crlf' &&
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
      await api.openChangedFile(thread.id, selectedPath);
    } catch (err) {
      showNotice(`Could not open file: ${errorMessage(err)}`);
    }
  }

  async function loadFile(path: string) {
    const seq = ++fileSeq;
    const threadId = thread.id;
    fileLoading = true;
    fileError = null;
    try {
      const f = await api.gitFile(threadId, path);
      if (seq !== fileSeq || selectedPath !== path || thread.id !== threadId) return;
      gitFile = f;
      fileEol = { orig: detectEol(f.old), cur: detectEol(f.current) };
    } catch (err) {
      if (seq !== fileSeq || selectedPath !== path || thread.id !== threadId) return;
      gitFile = null;
      fileError = errorMessage(err);
    } finally {
      if (seq === fileSeq) fileLoading = false;
    }
  }

  function selectFile(path: string) {
    if (selectedPath === path) return;
    selectedPath = path;
    gitFile = null;
    fileError = null;
    void loadFile(path);
  }
  /** Reload the changed-file list and the open file. Exported as a refresh hook. */
  export async function refresh() {
    const seq = ++refreshSeq;
    refreshing = true;
    try {
      const s = await api.gitStatus(thread.id);
      if (seq !== refreshSeq) return;
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
      const requestedPath =
        focusPath && focusPath !== lastFocus && s.files.some((f) => f.path === focusPath)
          ? focusPath
          : undefined;
      if (!selectedPath) {
        const target = requestedPath ?? s.files[0]?.path ?? null;
        if (target) selectFile(target);
      } else if (requestedPath) {
        selectFile(requestedPath);
      } else {
        void loadFile(selectedPath);
      }
      if (requestedPath) lastFocus = requestedPath;
    } catch (err) {
      if (seq === refreshSeq) error = errorMessage(err);
    } finally {
      if (seq === refreshSeq) {
        loading = false;
        refreshing = false;
      }
    }
  }

  // Watcher-driven refreshes run one at a time: on a large repo `git status`
  // can outlast the watcher interval, and overlapping runs pile up processes.
  let watchRefreshInFlight = false;
  let watchRefreshQueued = false;
  let destroyed = false;

  function scheduleRefresh() {
    clearTimeout(refreshTimer);
    refreshTimer = setTimeout(runWatchRefresh, 250);
  }

  function runWatchRefresh() {
    if (destroyed) return;
    if (watchRefreshInFlight) {
      watchRefreshQueued = true;
      return;
    }
    watchRefreshInFlight = true;
    void refresh().finally(() => {
      watchRefreshInFlight = false;
      if (watchRefreshQueued && !destroyed) {
        watchRefreshQueued = false;
        scheduleRefresh();
      }
    });
  }

  async function requestFileRevert() {
    if (!selectedPath || !selectedFile) return;
    const path = selectedPath;
    const threadId = thread.id;
    const meta = statusMeta(selectedFile.status);
    const untracked = meta.cls === 'added';
    const baseDetail = untracked
      ? 'This file is not tracked by git. Reverting deletes it from the working tree.'
      : `All uncommitted changes in this file are discarded (${meta.label}). This cannot be undone.`;
    const loaded = gitFile && gitFile.path === path ? gitFile : null;
    const loadedHash = loaded?.currentHash || null;
    // Hash guard: re-read the worktree bytes before confirming so the revert
    // is verified against the current on-disk state, not the stale open diff.
    let freshHash: string | null = null;
    let guardUnavailable = false;
    try {
      const fresh = await api.gitFile(threadId, path);
      if (selectedPath !== path || thread.id !== threadId) return;
      freshHash = fresh.currentHash || null;
      if (!freshHash) guardUnavailable = true;
    } catch {
      if (selectedPath !== path || thread.id !== threadId) return;
      guardUnavailable = true;
    }
    if (!guardUnavailable && loadedHash && freshHash && freshHash !== loadedHash) {
      // The worktree moved under the open diff: resync and make the user
      // confirm again on the fresh state instead of reverting what was shown.
      showNotice(`${path} changed on disk. Review the updated diff and confirm again.`);
      await refresh();
      return;
    }
    confirm = {
      title: 'Revert file to HEAD?',
      path,
      detail: guardUnavailable
        ? `${baseDetail} The current on-disk state could not be verified (no hash available), so the revert proceeds without a hash guard.`
        : baseDetail,
      confirmLabel: 'Revert file',
      run: async () => {
        await api.gitRevertFile(threadId, path, freshHash);
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
    const requestedPath = focusPath;
    if (requestedPath && requestedPath !== lastFocus && summary?.files.some((f) => f.path === requestedPath)) {
      lastFocus = requestedPath;
      selectFile(requestedPath);
    }
  });

  onMount(() => {
    try {
      // Read the old preference once; subsequent changes use πDesk's key.
      const saved = localStorage.getItem(MODE_KEY) ?? localStorage.getItem('omp.changes.diffMode');
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
      destroyed = true;
      refreshSeq += 1;
      fileSeq += 1;
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
      <span class="title-text">Changes</span>
      {#if summary?.branch}
        <span class="branch" title="Current branch"><GitBranch size={11} strokeWidth={2} />{summary.branch}</span>
      {/if}
    </div>
    <div class="panel-actions">
      {#if summary && summary.isRepo && summary.files.length > 0}
        <div class="mode-toggle" role="group" aria-label="Diff mode">
          <button
            type="button"
            class:active={mode === 'unified'}
            title="Unified diff"
            aria-label="Unified diff"
            aria-pressed={mode === 'unified'}
            onclick={() => setMode('unified')}
          >
            <List size={13} />
          </button>
          <button
            type="button"
            class:active={mode === 'split'}
            title="Split diff"
            aria-label="Split diff"
            aria-pressed={mode === 'split'}
            onclick={() => setMode('split')}
          >
            <Columns2 size={13} />
          </button>
        </div>
      {/if}
      <button
        type="button"
        class="icon-btn"
        title="Refresh changes"
        aria-label="Refresh changes"
        onclick={() => void refresh()}
        disabled={refreshing}
      >
        <RefreshCcw size={14} class={refreshing ? 'spin' : ''} />
      </button>
      <button type="button" class="icon-btn" title="Close changes" aria-label="Close changes" onclick={onClose}>
        <X size={15} />
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
          <VList data={summary.files} getKey={(f: ChangedFile) => f.path} itemSize={30}>
            {#snippet children(f: ChangedFile)}
              {@const meta = statusMeta(f.status)}
              {@const slash = f.path.lastIndexOf('/')}
              <button
                type="button"
                class="file-row"
                class:selected={f.path === selectedPath}
                aria-current={f.path === selectedPath ? 'true' : undefined}
                onclick={() => selectFile(f.path)}
                title={f.path}
              >
                <span class="status status-{meta.cls}" title={meta.label}>{meta.code}</span>
                <span class="file-path"><span class="file-name">{f.path.slice(slash + 1)}</span>{#if slash > 0}<span class="file-dir"><bdi>{f.path.slice(0, slash)}</bdi></span>{/if}</span>
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

      <div class="diff-pane" class:overlay={expanded}>
        {#if selectedPath && selectedFile}
          <div class="diff-head">
            <div class="diff-path" title={selectedPath}>
              <span class="status status-{statusMeta(selectedFile.status).cls}">
                {statusMeta(selectedFile.status).code}
              </span>
              <span class="diff-path-text"><bdi>{selectedPath}</bdi></span>
            </div>
            <div class="diff-actions">
              <button
                type="button"
                class="icon-btn"
                title={expanded ? 'Collapse review' : 'Expand review'}
                aria-label={expanded ? 'Collapse review' : 'Expand review'}
                onclick={() => (expanded = !expanded)}
              >
                {#if expanded}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
              </button>
              {#if diffable}
                <button type="button" class="icon-btn" title="Previous change" onclick={() => diffEditor?.prevChange()}>
                  <ChevronUp size={14} />
                </button>
                <button type="button" class="icon-btn" title="Next change" onclick={() => diffEditor?.nextChange()}>
                  <ChevronDown size={14} />
                </button>
              {/if}
              <button
                type="button"
                class="icon-btn"
                title={copied === 'selection' ? 'Copied' : 'Copy selection'}
                onclick={() => void copySelection()}
                disabled={!diffable}
              >
                <Copy size={14} />
              </button>
              <button
                type="button"
                class="icon-btn"
                title={copied === 'path' ? 'Copied' : 'Copy file path'}
                onclick={() => void copyPath()}
              >
                <span class="copy-path-label">{copied === 'path' ? '✓' : ''}</span>
                <FileDiff size={14} />
              </button>
              <button type="button" class="icon-btn" title="Open file externally" onclick={() => void openExternally()}>
                <ExternalLink size={14} />
              </button>
              <button
                type="button"
                class="icon-btn danger"
                title="Revert file to HEAD"
                onclick={() => void requestFileRevert()}
                disabled={confirmBusy}
              >
                <Undo2 size={14} />
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
            {#if !hunkRevertable && diffable && (fileEol.cur === 'mixed' || fileEol.cur === 'crlf' || fileEol.orig === 'mixed' || fileEol.orig === 'crlf')}
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
    background: var(--panel);
    color: var(--text);
    font-size: 12.5px;
    overflow: hidden;
  }
  .panel-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    height: var(--header-height);
    min-height: var(--header-height);
    padding: 0 10px 0 16px;
    border-bottom: 1px solid var(--line);
    flex: none;
  }
  .panel-title {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .title-text {
    font-size: 13px;
    font-weight: 600;
  }
  .branch {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 20px;
    padding: 0 7px;
    border-radius: 5px;
    background: var(--surface-2);
    color: var(--muted);
    font-family: var(--mono);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .branch :global(svg) {
    flex: none;
  }
  .panel-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: none;
  }
  .mode-toggle {
    display: flex;
    gap: 2px;
    padding: 2px;
    border-radius: 7px;
    background: var(--surface);
    border: 1px solid var(--line);
    margin-right: 6px;
  }
  .mode-toggle button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 22px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--subtle);
    cursor: pointer;
  }
  .mode-toggle button:hover {
    color: var(--text);
  }
  .mode-toggle button.active {
    background: var(--elevated);
    color: var(--text);
    box-shadow: var(--shadow-sm), 0 0 0 1px var(--line);
  }
  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2px;
    min-width: 28px;
    height: 28px;
    padding: 0 4px;
    border: none;
    border-radius: var(--radius-sm);
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
    background: var(--bad-bg);
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
  /* Narrow panel: file list stacked above the diff. */
  .panel-body.narrow {
    flex-direction: column;
  }
  .panel-body.narrow .file-pane {
    width: auto;
    max-height: 38%;
    min-height: 96px;
    border-right: none;
    border-bottom: 1px solid var(--line);
  }
  .file-pane {
    display: flex;
    flex-direction: column;
    width: 240px;
    min-width: 170px;
    flex: none;
    border-right: 1px solid var(--line);
    min-height: 0;
  }
  .file-pane.hidden {
    display: none;
  }
  .files-summary {
    padding: 10px 14px 6px;
    font-size: 11.5px;
    font-weight: 500;
    color: var(--muted);
    display: flex;
    justify-content: space-between;
    gap: 6px;
    flex: none;
  }
  .totals {
    display: flex;
    gap: 6px;
    white-space: nowrap;
    font-family: var(--mono);
    font-size: 11px;
  }
  .add { color: var(--good); }
  .del { color: var(--bad); }
  .file-list {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    padding: 0 6px 6px;
  }
  .file-row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 30px;
    padding: 0 8px;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
    text-align: left;
  }
  .file-row:hover {
    background: var(--surface);
  }
  .file-row.selected {
    background: var(--accent-bg);
  }
  .status {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    font-size: 9.5px;
    font-weight: 700;
    border-radius: 4px;
  }
  .status-added { color: var(--good); background: var(--good-bg); }
  .status-modified { color: var(--warn); background: var(--warn-bg); }
  .status-deleted { color: var(--bad); background: var(--bad-bg); }
  .status-renamed { color: var(--accent); background: var(--accent-bg); }
  .status-conflict { color: var(--bad); background: var(--bad-bg); }
  .status-other { color: var(--muted); background: var(--surface-2); }
  .file-path {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: baseline;
    gap: 6px;
    overflow: hidden;
    white-space: nowrap;
  }
  .file-name {
    flex: none;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .file-dir {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--subtle);
    font-size: 11.5px;
    direction: rtl;
    text-align: left;
  }
  .file-stats {
    flex: none;
    display: flex;
    gap: 5px;
    font-size: 11px;
    font-family: var(--mono);
  }
  .binary-tag {
    flex: none;
    font-size: 9.5px;
    text-transform: uppercase;
    color: var(--muted);
    border: 1px solid var(--line-strong);
    border-radius: 4px;
    padding: 0 4px;
  }

  .diff-pane {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    min-height: 0;
    background: var(--bg);
  }
  .diff-pane.overlay {
    position: fixed;
    top: 16px;
    right: 16px;
    bottom: 16px;
    left: 16px;
    z-index: 50;
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    overflow: hidden;
    animation: ui-pop 0.16s var(--ease);
  }
  .diff-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    height: 40px;
    padding: 0 6px 0 12px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
    flex: none;
  }
  .diff-path {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    font-family: var(--mono);
    font-size: 11.5px;
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
    gap: 0;
    flex: none;
  }
  .diff-actions .icon-btn {
    min-width: 26px;
    height: 26px;
  }
  .diff-note {
    flex: none;
    padding: 6px 12px;
    font-size: 11px;
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
    padding: 28px;
    text-align: center;
  }
  .state-msg.small {
    padding: 18px;
  }
  .state-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }
  .state-detail {
    font-size: 12px;
    max-width: 320px;
    line-height: 1.55;
  }
  .btn {
    font: inherit;
    font-size: 12px;
    font-weight: 500;
    height: 28px;
    padding: 0 12px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--line-strong);
    background: var(--elevated);
    color: var(--text);
    cursor: pointer;
  }
  .btn:hover {
    background: var(--surface-2);
  }
  .notice {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    bottom: 14px;
    max-width: calc(100% - 28px);
    padding: 8px 14px;
    font-size: 12px;
    color: var(--text);
    background: var(--elevated);
    border-radius: 999px;
    box-shadow: var(--shadow);
    z-index: 40;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    animation: ui-rise 0.16s var(--ease);
  }
</style>
