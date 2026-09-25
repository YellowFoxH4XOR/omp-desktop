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
      <span class="confirm-icon"><TriangleAlert size={15} strokeWidth={2} /></span>
      <span class="confirm-title">{title}</span>
    </div>
    <div class="confirm-path" title={path}><bdi>{path}</bdi></div>
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
    background: rgb(0 0 0 / 0.35);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 60;
    animation: fade-in 0.12s ease-out;
  }
  @keyframes fade-in {
    from { opacity: 0; }
    to { opacity: 1; }
  }
  .confirm {
    width: min(420px, calc(100vw - 48px));
    background: var(--elevated);
    border-radius: var(--radius-lg);
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    box-shadow: var(--shadow);
    animation: ui-pop 0.16s var(--ease);
  }
  .confirm-head {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .confirm-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    color: var(--bad);
    background: var(--bad-bg);
  }
  .confirm-title {
    font-size: 14px;
    font-weight: 600;
    color: var(--text);
  }
  .confirm-path {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 7px 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
  .confirm-detail {
    font-size: 12.5px;
    color: var(--muted);
    line-height: 1.55;
  }
  .confirm-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
  .btn {
    font: inherit;
    font-size: 12.5px;
    font-weight: 500;
    height: 30px;
    padding: 0 14px;
    border-radius: var(--radius);
    border: 1px solid var(--line-strong);
    background: var(--surface);
    color: var(--text);
    cursor: pointer;
  }
  .btn:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .btn.danger {
    border-color: transparent;
    background: var(--bad);
    color: #fff;
    font-weight: 600;
  }
  .btn.danger:hover:not(:disabled) {
    background: var(--bad);
    filter: brightness(1.08);
  }
</style>
