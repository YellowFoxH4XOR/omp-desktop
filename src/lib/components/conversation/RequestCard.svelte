<script lang="ts">
  import { ExternalLink, KeyRound, Shield } from '@lucide/svelte';
  import type { UiRequest, UiResponse } from '../../types';
  import { openExternal } from './links';
  import { prettyJson } from '../tools/tool-utils';

  interface Props {
    request: UiRequest;
    onRespond: (requestId: string, response: UiResponse) => void | Promise<void>;
  }

  let { request, onRespond }: Props = $props();

  let text = $state('');
  let answered = $state(false);
  let secondsLeft = $state<number | null>(null);
  let linkFeedback = $state('');

  const isSecret = $derived(/password|secret|token|api[-_ ]?key|credential|auth code|oauth/i.test(
    `${request.title} ${request.message ?? ''} ${request.placeholder ?? ''}`,
  ));

  $effect(() => {
    text = request.prefill ?? '';
    answered = false;
    linkFeedback = '';
  });

  $effect(() => {
    if (!request.timeout || request.timeout <= 0) {
      secondsLeft = null;
      return;
    }
    // Harness timeouts arrive in milliseconds.
    const deadline = Date.now() + request.timeout;
    secondsLeft = Math.ceil(request.timeout / 1000);
    const timer = setInterval(() => {
      secondsLeft = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
    }, 1000);
    return () => clearInterval(timer);
  });

  async function respond(response: UiResponse) {
    if (answered) return;
    answered = true;
    try { await onRespond(request.id, response); }
    catch { answered = false; }
  }

  /** Compact arg summary: scalar values inline, rest in the disclosure. */
  const argEntries = $derived(
    Object.entries(request.toolArgs ?? {}).filter(([, value]) => value !== undefined),
  );
  const scalarArgs = $derived(
    argEntries.filter(([, value]) =>
      ['string', 'number', 'boolean'].includes(typeof value),
    ) as Array<[string, string | number | boolean]>,
  );
  const complexArgs = $derived(argEntries.length - scalarArgs.length);
</script>

<div class="request" role="group" aria-label={`Request: ${request.title}`}>
  <div class="head">
    <span class="icon" aria-hidden="true">
      {#if request.method === 'open_url'}<ExternalLink size={13} />
      {:else if isSecret}<KeyRound size={13} />
      {:else}<Shield size={13} />{/if}
    </span>
    <span class="title">{request.title}</span>
    {#if secondsLeft !== null}
      <span class="timeout" aria-label="Time remaining">{secondsLeft}s</span>
    {/if}
  </div>

  {#if request.message}
    <p class="message">{request.message}</p>
  {/if}

  {#if request.method === 'open_url' && request.url}
    <div class="url-row">
      <code class="url">{request.url}</code>
    </div>
  {/if}
  {#if linkFeedback}<p class="link-feedback" role="status">{linkFeedback}</p>{/if}

  {#if request.cwd}
    <div class="kv"><span class="k">Working directory</span><code class="v">{request.cwd}</code></div>
  {/if}
  {#if request.toolName}
    <div class="kv"><span class="k">Operation</span><code class="v">{request.toolName}</code></div>
  {/if}
  {#each scalarArgs as [key, value] (key)}
    <div class="kv"><span class="k">{key}</span><code class="v">{String(value)}</code></div>
  {/each}
  {#if complexArgs > 0}
    <details class="raw">
      <summary>Arguments</summary>
      <pre>{prettyJson(request.toolArgs)}</pre>
    </details>
  {/if}

  <fieldset class="actions" disabled={answered}>
    {#if request.method === 'select' && request.options && request.options.length > 0}
      <div class="options">
        {#each request.options as option, i (option)}
          <button type="button" class="opt" onclick={() => respond({ value: option })}>
            <span class="opt-label">{option}</span>
            {#if request.optionDetails?.[i]?.description}
              <span class="opt-desc">{request.optionDetails[i].description}</span>
            {/if}
          </button>
        {/each}
      </div>
      <button type="button" class="btn ghost" onclick={() => respond({ cancelled: true })}>Cancel</button>
    {:else if request.method === 'input' || request.method === 'editor'}
      {#if request.method === 'editor'}
        <textarea
          class="field"
          rows="5"
          placeholder={request.placeholder ?? ''}
          bind:value={text}
          aria-label={request.title}
        ></textarea>
      {:else}
        <input
          class="field"
          type={isSecret ? 'password' : 'text'}
          placeholder={request.placeholder ?? ''}
          bind:value={text}
          aria-label={request.title}
          onkeydown={(e) => e.key === 'Enter' && respond({ value: text })}
        />
      {/if}
      <div class="row-actions">
        <button type="button" class="btn primary" onclick={() => respond({ value: text })}>Submit</button>
        <button type="button" class="btn ghost" onclick={() => respond({ cancelled: true })}>Cancel</button>
      </div>
    {:else if request.method === 'open_url'}
      <div class="row-actions">
        <button
          type="button"
          class="btn primary"
          onclick={async () => {
            const opened = await openExternal(request.launchUrl ?? request.url ?? '');
            if (opened) await respond({ confirmed: true });
            else linkFeedback = 'Could not open the link. Copy it and open it in your browser.';
          }}
        >
          Open link
        </button>
        <button type="button" class="btn ghost" onclick={async () => {
          try {
            await navigator.clipboard.writeText(request.launchUrl ?? request.url ?? '');
            linkFeedback = 'Link copied. Open it in your browser to continue sign-in.';
          } catch {
            linkFeedback = 'Clipboard unavailable; select and copy the URL above.';
          }
        }}>Copy link</button>
        <button type="button" class="btn ghost" onclick={() => respond({ cancelled: true })}>Dismiss</button>
      </div>
    {:else if request.options && request.options.length > 0}
      <div class="row-actions">
        {#each request.options as option, i (option)}
          {@const last = i === request.options.length - 1}
          <button
            type="button"
            class="btn"
            class:primary={last}
            class:ghost={!last}
            onclick={() => respond({ value: option })}
          >
            {option}
          </button>
        {/each}
      </div>
    {:else}
      <div class="row-actions">
        <button type="button" class="btn danger" onclick={() => respond(request.method === 'permission' ? { value: 'Deny' } : { confirmed: false })}>Deny</button>
        <button type="button" class="btn primary" onclick={() => respond(request.method === 'permission' ? { value: 'Approve' } : { confirmed: true })}>
          {request.method === 'permission' ? 'Approve' : 'Confirm'}
        </button>
        <button type="button" class="btn ghost" onclick={() => respond({ cancelled: true })}>Cancel</button>
      </div>
    {/if}
  </fieldset>
</div>

<style>
  .request {
    border: 1px solid var(--warn);
    border-radius: var(--radius);
    background: var(--surface);
    padding: 8px 10px;
    margin: 6px 0;
  }
  .actions {
    margin-top: 8px;
    border: 0;
    padding: 0;
    min-width: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .icon {
    display: inline-flex;
    color: var(--warn);
  }
  .title {
    flex: 1;
    font-weight: 600;
    font-size: 12.5px;
    min-width: 0;
  }
  .timeout {
    flex: none;
    color: var(--muted);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .message {
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--text);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .url-row {
    margin-top: 6px;
    padding: 5px 8px;
    border-radius: 6px;
    background: var(--bg);
    overflow-x: auto;
  }
  .url {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--accent);
    white-space: nowrap;
  }
  .link-feedback { margin: 6px 0 0; color: var(--warn); font-size: 11px; }
  .kv {
    display: flex;
    gap: 8px;
    align-items: baseline;
    margin-top: 5px;
    font-size: 11.5px;
    min-width: 0;
  }
  .k {
    flex: none;
    color: var(--muted);
    min-width: 110px;
  }
  .v {
    font-family: var(--mono);
    font-size: 11px;
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .raw {
    margin-top: 5px;
  }
  .raw summary {
    cursor: pointer;
    color: var(--muted);
    font-size: 11px;
  }
  .raw pre {
    margin: 4px 0 0;
    padding: 6px 8px;
    border-radius: 6px;
    background: var(--bg);
    font-family: var(--mono);
    font-size: 11px;
    overflow-x: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .actions {
    margin-top: 8px;
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-bottom: 6px;
  }
  .opt {
    display: flex;
    flex-direction: column;
    gap: 1px;
    padding: 5px 8px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--text);
    font-size: 12px;
    text-align: left;
  }
  .opt:hover {
    border-color: var(--accent);
  }
  .opt-desc {
    color: var(--muted);
    font-size: 11px;
  }
  .row-actions {
    display: flex;
    gap: 6px;
    justify-content: flex-end;
    flex-wrap: wrap;
  }
  .btn {
    padding: 4px 12px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--text);
    font-size: 12px;
  }
  .btn:hover {
    background: var(--surface-3);
  }
  .btn.primary {
    background: var(--accent-bg);
    border-color: transparent;
    color: var(--text);
  }
  .btn.danger {
    color: var(--bad);
  }
  .btn.ghost {
    background: transparent;
    color: var(--muted);
  }
  .field {
    width: 100%;
    padding: 6px 8px;
    border: 1px solid var(--line);
    border-radius: 6px;
    background: var(--bg);
    color: var(--text);
    font-size: 12px;
    font-family: inherit;
    resize: vertical;
    margin-bottom: 6px;
  }
</style>
