<script lang="ts">
  import { ClipboardList, Check, MessageSquareText, X, Zap } from '@lucide/svelte';
  import Markdown from '../conversation/Markdown.svelte';

  interface Props {
    plan: string;
    /** Who proposed it, e.g. the thread title or "Pi Intern". */
    source?: string;
    onApprove: () => void | Promise<void>;
    onDecline: () => void | Promise<void>;
    onFeedback: (text: string) => void | Promise<void>;
    onClose?: () => void;
  }

  let { plan, source, onApprove, onDecline, onFeedback, onClose }: Props = $props();

  let busy = $state(false);
  let feedbackOpen = $state(false);
  let feedback = $state('');
  let feedbackField = $state<HTMLTextAreaElement>();

  // A leading Markdown heading becomes the panel title.
  const heading = $derived(plan.match(/^\s*#{1,3}\s+(.+)\n?/)?.[1]?.trim() ?? 'Proposed plan');
  const body = $derived(plan.replace(/^\s*#{1,3}\s+.+\n?/, '').trim() || plan);

  async function act(run: () => void | Promise<void>) {
    if (busy) return;
    busy = true;
    try { await run(); } finally { busy = false; }
  }
  function openFeedback() {
    feedbackOpen = true;
    queueMicrotask(() => feedbackField?.focus());
  }
  function sendFeedback() {
    const text = feedback.trim();
    if (text) void act(() => onFeedback(text));
  }
  function onKey(event: KeyboardEvent) {
    if (!event.metaKey || event.key !== 'Enter') return;
    event.preventDefault();
    if (feedbackOpen) sendFeedback();
    else void act(onApprove);
  }
</script>

<div class="plan-review" role="dialog" aria-modal="false" tabindex="-1" aria-label="Plan review" onkeydown={onKey}>
  <header>
    <span class="glyph" aria-hidden="true"><ClipboardList size={17} strokeWidth={1.9} /></span>
    <div class="titles">
      <span class="eyebrow"><span class="pulse" aria-hidden="true"></span>Awaiting your approval{#if source}{` · ${source}`}{/if}</span>
      <h2>{heading}</h2>
    </div>
    {#if onClose}<button class="close" title="Hide (the plan stays pending)" aria-label="Hide plan review" onclick={onClose}><X size={16} /></button>{/if}
  </header>

  <div class="scroll">
    <article class="doc"><Markdown text={body} /></article>
  </div>

  <footer>
    {#if feedbackOpen}
      <label class="feedback">
        <span>What should change?</span>
        <textarea bind:this={feedbackField} bind:value={feedback} rows="3" maxlength="8000" placeholder="e.g. Skip the migration, and add a test for the empty case" disabled={busy}></textarea>
      </label>
      <div class="actions">
        <button class="ghost" disabled={busy} onclick={() => { feedbackOpen = false; feedback = ''; }}>Cancel</button>
        <button class="primary" disabled={busy || !feedback.trim()} onclick={sendFeedback}><MessageSquareText size={14} strokeWidth={2} />Send feedback<kbd>⌘↵</kbd></button>
      </div>
    {:else}
      <p class="note"><Zap size={12} strokeWidth={2} /><span>Approving runs this plan with full tools for one run, then returns to Plan mode. <kbd>⌘↵</kbd> approves.</span></p>
      <div class="actions">
        <button class="ghost danger" disabled={busy} onclick={() => void act(onDecline)}>Decline</button>
        <button class="secondary" disabled={busy} onclick={openFeedback}><MessageSquareText size={14} strokeWidth={2} />Give feedback</button>
        <button class="primary" disabled={busy} onclick={() => void act(onApprove)}><Check size={14} strokeWidth={2.4} />Approve &amp; run</button>
      </div>
    {/if}
  </footer>
</div>

<style>
  .plan-review {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--panel);
  }
  header {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 18px 20px 16px;
    border-bottom: 1px solid var(--line);
  }
  .glyph {
    flex: none;
    width: 34px;
    height: 34px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 10px;
    color: var(--accent);
    background: var(--accent-bg);
  }
  .titles {
    flex: 1;
    min-width: 0;
  }
  .eyebrow {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--warn);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.02em;
  }
  .pulse {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: currentColor;
    animation: pulse 1.6s ease-in-out infinite;
  }
  @keyframes pulse {
    50% { opacity: 0.35; }
  }
  h2 {
    margin: 4px 0 0;
    font-size: 17px;
    font-weight: 600;
    letter-spacing: -0.015em;
    line-height: 1.3;
    overflow-wrap: anywhere;
  }
  .close {
    flex: none;
    display: flex;
    padding: 5px;
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    color: var(--muted);
  }
  .close:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .doc {
    max-width: 720px;
    margin: 0 auto;
    padding: 20px 24px 28px;
    font-size: 14px;
    line-height: 1.65;
  }
  footer {
    flex: none;
    padding: 14px 20px 16px;
    border-top: 1px solid var(--line);
    background: var(--elevated);
  }
  .note :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .note kbd {
    background: var(--surface-2);
  }
  .note {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin: 0 0 12px;
    color: var(--muted);
    font-size: 12px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }
  .actions button {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    padding: 0 12px;
    border-radius: var(--radius);
    white-space: nowrap;
    font: 600 13px var(--font);
    transition: filter 0.12s, background 0.12s;
  }
  .actions button:disabled {
    opacity: 0.5;
  }
  .primary {
    border: 0;
    background: var(--accent-strong);
    color: var(--on-accent);
    box-shadow: var(--shadow-sm);
  }
  .primary:hover:not(:disabled) {
    filter: brightness(1.08);
  }
  .secondary {
    border: 1px solid var(--line-strong);
    background: var(--surface);
    color: var(--text);
  }
  .secondary:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .ghost {
    margin-right: auto;
    border: 0;
    background: none;
    color: var(--muted);
  }
  .ghost:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-2);
  }
  .ghost.danger:hover:not(:disabled) {
    color: var(--bad);
    background: var(--bad-bg);
  }
  kbd {
    border: 0;
    min-width: 0;
    height: auto;
    margin-left: 2px;
    padding: 0 4px;
    border-radius: 4px;
    background: rgb(255 255 255 / 0.18);
    color: inherit;
    font-size: 10.5px;
    font-family: var(--font);
  }
  .feedback {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 10px;
    font-size: 12px;
    font-weight: 600;
    color: var(--muted);
  }
  .feedback textarea {
    resize: vertical;
    min-height: 64px;
    max-height: 200px;
    padding: 9px 10px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font: 400 13px/1.5 var(--font);
  }
  .feedback textarea:focus {
    outline: none;
    border-color: var(--accent);
    box-shadow: var(--focus-ring);
  }
  button:focus-visible {
    outline: none;
    box-shadow: var(--focus-ring);
  }
</style>
