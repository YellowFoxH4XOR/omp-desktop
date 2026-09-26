<script lang="ts">
  import { tick } from 'svelte';
  import { AlertTriangle, ArrowDown, ArrowRight, Check, Download, Folder, LoaderCircle, RefreshCw, ShieldCheck, Terminal } from '@lucide/svelte';
  import type { HarnessInstallCommand, InstallStatus } from '../../types';
  import PiSignIn from './PiSignIn.svelte';

  interface Props {
    plan: HarnessInstallCommand | null;
    status: InstallStatus;
    busy: boolean;
    lines: string[];
    error: string;
    ready: boolean;
    checking: boolean;
    onInstall: () => void;
    onCheck: () => void;
    onContinue: () => void;
    onOpenTerminal: () => void;
  }
  let { plan, status, busy, lines, error, ready, checking, onInstall, onCheck, onContinue, onOpenTerminal }: Props = $props();
  let output = $state<HTMLDivElement>();
  let following = $state(true);
  const steps = ['Check requirements', 'Install packages', 'Verify Pi'];
  const stepIndex = $derived(status === 'preparing' ? 0 : status === 'installing' ? 1 : status === 'verifying' ? 2 : status === 'complete' ? 3 : -1);
  const statusLabel = $derived(status === 'idle' ? 'Not installed' : status === 'preparing' ? 'Checking requirements' : status === 'installing' ? 'Installing packages' : status === 'verifying' ? 'Verifying installation' : status === 'complete' ? 'Ready' : 'Installation failed');

  $effect(() => {
    lines;
    if (following) void tick().then(() => { if (output) output.scrollTop = output.scrollHeight; });
  });
  function onScroll() {
    if (output) following = output.scrollHeight - output.scrollTop - output.clientHeight < 32;
  }
</script>

<section class="setup" aria-label="Private Pi setup">
  <div class="setup-content">
    <header>
      <div class="eyebrow"><span class="pi-mark" aria-hidden="true">π</span><span>πDesk / SETUP</span></div>
      <h1>{status === 'complete' ? 'Your Pi is ready.' : 'A Pi of its own.'}</h1>
      <p class="intro">{status === 'complete' ? 'Installed just for πDesk. Connect a provider below, then you’re ready to work.' : 'Install a dedicated Pi for πDesk. Your terminal’s Pi, extensions, and sessions stay untouched.'}</p>
    </header>

    <div class="location-card">
      <div class="location-title"><ShieldCheck size={17} strokeWidth={1.7} /><strong>Separate by design</strong><span class="local-badge">LOCAL</span></div>
      <div class="location-row"><Terminal size={14} /><span>Installation</span><code title={plan?.installPath}>{plan?.installPath ?? '~/.pidesk/runtime'}</code></div>
      <div class="location-row"><Folder size={14} /><span>Private data</span><code title={plan?.agentDir}>{plan?.agentDir ?? '~/.pidesk/agent'}</code></div>
      <p>Settings, sign-in, extensions, and new sessions belong to this copy. Existing projects stay in place.</p>
    </div>

    <ol class="steps" aria-label="Installation steps">
      {#each steps as label, index}
        <li class:done={stepIndex > index} class:active={stepIndex === index} aria-current={stepIndex === index ? 'step' : undefined}>
          <span class="step-number">{#if stepIndex > index}<Check size={12} />{:else}{index + 1}{/if}</span><span>{label}</span>
        </li>
      {/each}
    </ol>

    <div class="terminal" class:failed={status === 'failed'}>
      <div class="terminal-header">
        <span class="dots" aria-hidden="true"><i></i><i></i><i></i></span>
        <span class="terminal-name">pi · setup</span>
        <span class="terminal-status" class:success={status === 'complete'} role="status">
          {#if busy}<LoaderCircle size={12} class="spin" />{:else if status === 'complete'}<Check size={12} />{:else if status === 'failed'}<AlertTriangle size={12} />{/if}{statusLabel}
        </span>
      </div>
      <!-- svelte-ignore a11y_no_noninteractive_tabindex (The scrollable log needs keyboard focus for scrolling and text selection.) -->
      <div class="terminal-output" bind:this={output} onscroll={onScroll} role="log" aria-label="Pi installation output" aria-live="polite" aria-relevant="additions" tabindex="0">
        {#if status === 'idle'}
          <pre class="command">$ {plan?.command ?? 'Loading installer…'}</pre>
          <p class="waiting">Ready when you are. Nothing runs until you choose Install Pi.</p>
        {:else}
          <pre>{#each lines as line}<span>{line}{'\n'}</span>{/each}{#if busy}<span class="cursor" aria-hidden="true">▍</span>{/if}</pre>
        {/if}
      </div>
      {#if !following && busy}<button class="follow" onclick={() => following = true}><ArrowDown size={12} /> Follow output</button>{/if}
      <div class="terminal-footer"><span>Live output · latest 200 lines</span><span>No global install</span></div>
    </div>

    {#if error}<div class="install-error" role="alert"><AlertTriangle size={16} /><div><strong>{status === 'failed' ? 'Installation didn’t finish' : 'Setup is unavailable'}</strong><p>{error}</p><small>Your existing Pi installation has not been changed.</small></div></div>{/if}

    {#if status === 'complete' && plan}<PiSignIn command={plan.loginCommand} {onOpenTerminal} />{/if}

    <div class="actions">
      {#if status === 'complete'}
        <button class="primary" disabled={busy} onclick={onContinue}>Continue to πDesk <ArrowRight size={15} /></button>
      {:else}
        <button class="primary" disabled={busy || !ready || checking} onclick={onInstall}>
          {#if busy}<LoaderCircle size={15} class="spin" />{statusLabel}…{:else if status === 'failed'}<RefreshCw size={15} />Retry installation{:else}<Download size={15} />Install Pi{/if}
        </button>
        <button class="secondary" disabled={busy || checking} onclick={onCheck}><RefreshCw size={13} class={checking ? 'spin' : ''} />{checking ? 'Checking…' : 'Check again'}</button>
      {/if}
    </div>
    <p class="footnote">{busy ? 'Keep πDesk open while the installation runs. You can review the output above.' : 'Requires Node.js 22.19+ and npm. Installation starts only when you choose it.'}</p>
  </div>
</section>

<style>
  .setup { flex:1; min-height:0; overflow:auto; padding:24px 28px; }
  .setup-content { max-width:660px; margin:auto; display:flex; flex-direction:column; gap:16px; }
  .eyebrow { display:flex; align-items:center; gap:10px; color:var(--subtle); font-size:10px; font-weight:600; letter-spacing:.12em; }
  .pi-mark { display:flex; align-items:center; justify-content:center; width:32px; height:32px; border-radius:9px; background:var(--accent-bg); color:var(--accent); font-size:25px; font-weight:600; letter-spacing:0; }
  h1 { margin:17px 0 8px; font-size:29px; font-weight:600; letter-spacing:-.035em; }
  .intro { margin:0; max-width:520px; color:var(--muted); font-size:13px; line-height:1.7; }
  .location-card { border:1px solid var(--line); background:var(--surface); border-radius:12px; padding:16px; }
  .location-title { display:flex; align-items:center; gap:8px; color:var(--accent); margin-bottom:9px; }
  .location-title strong { color:var(--text); font-size:12px; font-weight:600; }
  .local-badge { margin-left:auto; font-size:9px; font-weight:650; letter-spacing:.08em; color:var(--subtle); }
  .location-row { display:flex; align-items:center; gap:8px; min-width:0; padding:5px 0; color:var(--muted); font-size:11.5px; }
  .location-row :global(svg) { flex:none; color:var(--subtle); }
  .location-row > span { width:84px; flex:none; }
  .location-row code { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font:11px var(--mono); color:var(--text); }
  .location-card p { margin:10px 0 0; padding-top:11px; border-top:1px solid var(--line); color:var(--subtle); font-size:11.5px; line-height:1.6; }
  .steps { display:flex; gap:18px; list-style:none; margin:0; padding:0; }
  .steps li { display:flex; align-items:center; gap:7px; color:var(--subtle); font-size:11px; }
  .step-number { display:flex; align-items:center; justify-content:center; width:21px; height:21px; border:1px solid var(--line-strong); border-radius:50%; font-size:10px; }
  .steps .active { color:var(--accent); }
  .active .step-number { border-color:var(--accent); background:var(--accent-bg); }
  .steps .done { color:var(--good); }
  .done .step-number { border-color:transparent; background:var(--good-bg); }
  .terminal { position:relative; overflow:hidden; border:1px solid #303443; border-radius:11px; color:#d1d5e0; background:#12151c; box-shadow:0 6px 20px rgb(0 0 0 / .08); }
  .terminal.failed { border-color:var(--bad); }
  .terminal-header { display:flex; align-items:center; gap:11px; padding:11px 14px; background:#1a1e28; border-bottom:1px solid #2a2f3c; font-size:10.5px; }
  .dots { display:flex; gap:4px; }
  .dots i { width:7px; height:7px; border-radius:50%; background:#465064; }
  .terminal-name { color:#929bae; font-family:var(--mono); }
  .terminal-status { margin-left:auto; display:flex; align-items:center; gap:5px; color:#b4befa; }
  .terminal-status.success { color:#9bd3ac; }
  .terminal-output { height:135px; padding:16px; overflow:auto; scrollbar-color:#394252 transparent; }
  pre { margin:0; color:inherit; font:11px/1.85 var(--mono); white-space:pre-wrap; overflow-wrap:anywhere; }
  pre span { white-space:pre-wrap; }
  .command { color:#c7cffb; }
  .waiting { margin:15px 0 0; color:#909aad; font:11px/1.7 var(--mono); }
  .cursor { color:#b4befa; }
  .terminal-footer { display:flex; justify-content:space-between; padding:8px 14px; border-top:1px solid #252b37; color:#909aad; font-size:10px; }
  .follow { position:absolute; bottom:39px; right:14px; display:flex; align-items:center; gap:5px; padding:4px 8px; background:#292f40; border:1px solid #47516a; border-radius:5px; color:#d1d5e0; font-size:11px; }
  .install-error { display:flex; align-items:flex-start; gap:10px; padding:13px; border:1px solid color-mix(in srgb, var(--bad) 25%, transparent); background:var(--bad-bg); border-radius:9px; color:var(--bad); }
  .install-error :global(svg) { flex:none; margin-top:1px; }
  .install-error strong { font-size:12px; }
  .install-error p { margin:5px 0; font-size:12px; line-height:1.6; overflow-wrap:anywhere; white-space:pre-wrap; }
  .install-error small { color:var(--muted); font-size:11px; }
  .actions { display:flex; gap:12px; align-items:center; }
  .actions button { display:inline-flex; gap:7px; align-items:center; justify-content:center; border-radius:7px; padding:0 15px; height:35px; font-size:12px; font-weight:600; }
  .primary { color:var(--on-accent); background:var(--accent-strong); border:0; }
  .primary:hover:not(:disabled) { filter:brightness(1.08); }
  .secondary { background:transparent; border:1px solid var(--line-strong); color:var(--muted); }
  .secondary:hover:not(:disabled) { background:var(--surface); color:var(--text); }
  .footnote { color:var(--subtle); font-size:11px; line-height:1.6; margin:-9px 0 0; }
  @media (max-width:1050px) { .steps { gap:12px; flex-wrap:wrap; } .setup { padding:22px 20px; } }
</style>
