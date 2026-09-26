<script lang="ts">
  import { Check, Copy, Terminal } from '@lucide/svelte';

  let { command }: { command: string } = $props();
  let copied = $state(false);
  let copyError = $state('');

  async function copy() {
    try {
      await navigator.clipboard.writeText(command);
      copied = true;
      copyError = '';
    } catch {
      copyError = 'Select and copy the command below instead.';
    }
  }
</script>

<div class="sign-in">
  <div class="sign-in-heading"><Terminal size={15} /><h3>Connect your provider</h3></div>
  <p>Run this command in Terminal, then use <code>/login</code>. This opens πDesk’s private Pi—not your usual installation.</p>
  <div class="command-row">
    <textarea aria-label="Private Pi sign-in command" readonly value={command} rows="3" spellcheck="false"></textarea>
    <button type="button" onclick={() => void copy()} aria-label="Copy private Pi command" title="Copy private Pi command">
      {#if copied}<Check size={14} />{:else}<Copy size={14} />{/if}
    </button>
  </div>
  <span class="copy-status" role="status">{copyError || (copied ? 'Command copied.' : 'Credentials are stored separately for πDesk.')}</span>
</div>

<style>
  .sign-in { padding:16px; border:1px solid var(--line); border-radius:var(--radius); background:var(--surface); }
  .sign-in-heading { display:flex; align-items:center; gap:8px; color:var(--accent); }
  h3 { margin:0; color:var(--text); font-size:13px; font-weight:600; }
  p { margin:8px 0 12px; font-size:12px; line-height:1.6; color:var(--muted); }
  code { color:var(--text); }
  .command-row { display:flex; gap:6px; align-items:flex-start; padding:8px; background:var(--bg); border:1px solid var(--line); border-radius:6px; }
  textarea { width:100%; min-width:0; resize:vertical; background:transparent; border:0; font:11px/1.5 var(--mono); color:var(--muted); padding:0; }
  button { flex:none; display:flex; align-items:center; justify-content:center; width:26px; height:26px; border:0; border-radius:5px; color:var(--muted); background:var(--surface-2); }
  button:hover { color:var(--text); }
  .copy-status { display:block; min-height:16px; margin-top:7px; font-size:11px; color:var(--subtle); }
</style>
