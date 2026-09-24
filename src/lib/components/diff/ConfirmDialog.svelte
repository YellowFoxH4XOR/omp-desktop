<script lang="ts">
  import { onMount } from 'svelte';
  import { TriangleAlert } from '@lucide/svelte';

  /** In-app confirmation for destructive git actions (file/hunk revert). */
  let {
    title,
    path,
    detail,
    confirmLabel = 'Revert',
    busy = false,
    onConfirm,
    onCancel,
  }: {
    title: string;
    path: string;
    detail?: string;
    confirmLabel?: string;
    busy?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  } = $props();

  let dialogEl: HTMLDivElement | undefined = $state();
  let cancelEl: HTMLButtonElement | undefined = $state();

  onMount(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    cancelEl?.focus();

    return () => {
      if (previousFocus?.isConnected) previousFocus.focus();
    };
  });

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      if (!busy) onCancel();
      return;
    }
    if (e.key !== 'Tab') return;

    const controls = dialogEl
      ? Array.from(dialogEl.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'))
      : [];
    if (controls.length === 0) {
      e.preventDefault();
      dialogEl?.focus();
      return;
    }

    const first = controls[0];
    const last = controls[controls.length - 1];
    if (e.shiftKey && (document.activeElement === first || !dialogEl?.contains(document.activeElement))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="confirm-backdrop" role="presentation" onmousedown={(e) => { if (e.target === e.currentTarget && !busy) onCancel(); }}>
  <div bind:this={dialogEl} class="confirm" role="alertdialog" aria-modal="true" aria-label={title} tabindex="-1">
    <div class="confirm-head">
      <TriangleAlert size={14} />
      <span class="confirm-title">{title}</span>
    </div>
    <div class="confirm-path" title={path}>{path}</div>
    {#if detail}
      <div class="confirm-detail">{detail}</div>
    {/if}
    <div class="confirm-actions">
      <button bind:this={cancelEl} type="button" class="btn" onclick={onCancel} disabled={busy}>Cancel</button>
      <button type="button" class="btn danger" onclick={onConfirm} disabled={busy}>
        {busy ? 'Working…' : confirmLabel}
      </button>
    </div>
  </div>
</div>

<style>
  .confirm-backdrop {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--bg) 55%, transparent);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 60;
  }
  .confirm {
    width: min(420px, calc(100vw - 48px));
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    box-shadow: 0 12px 40px color-mix(in srgb, var(--bg) 70%, transparent);
  }
  .confirm-head {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--warn);
  }
  .confirm-title {
    font-size: 12px;
    font-weight: 600;
    color: var(--text);
  }
  .confirm-path {
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 11px;
    color: var(--text);
    background: var(--surface-2);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 5px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .confirm-detail {
    font-size: 11px;
    color: var(--muted);
    line-height: 1.5;
  }
  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 4px;
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
  .btn:hover:not(:disabled) {
    border-color: var(--muted);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .btn.danger {
    border-color: var(--bad);
    color: var(--bad);
  }
  .btn.danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--bad) 15%, transparent);
  }
</style>
