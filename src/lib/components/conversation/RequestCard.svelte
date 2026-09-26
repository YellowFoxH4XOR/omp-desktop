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
  let responding = false;

  const isSecret = $derived(/password|secret|token|api[-_ ]?key|credential|auth code|oauth/i.test(
    `${request.title} ${request.message ?? ''} ${request.placeholder ?? ''}`,
  ));

  $effect(() => {
    text = request.prefill ?? '';
    answered = false;
    responding = false;
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
    const requestId = request.id;
    if (answered || responding) return;
    answered = true;
    responding = true;
    try { await onRespond(requestId, response); }
    catch {
      if (request.id === requestId) answered = false;
    } finally {
      if (request.id === requestId) responding = false;
    }
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
  // Permission titles embed the command after a newline; the details below show it.
  const heading = $derived(
    request.method === 'permission' && request.toolName
      ? `Allow ${request.toolName}?`
      : request.title.split('\n')[0],
  );
  // Other multi-line titles (such as a Plan → Auto approval) carry their body
  // after the first line; show it instead of dropping it.
  const titleBody = $derived(
    request.method === 'permission' ? '' : request.title.split('\n').slice(1).join('\n').trim(),
  );
  const kindLabel = $derived(
    request.method === 'permission' ? 'Permission needed' : request.method === 'open_url' ? 'Sign-in' : 'Input needed',
  );
</script>

<div class="request" role="group" aria-label={`Request: ${request.title}`}>
  <div class="head">
    <span class="icon" aria-hidden="true">
      {#if request.method === 'open_url'}<ExternalLink size={15} strokeWidth={2} />
      {:else if isSecret}<KeyRound size={15} strokeWidth={2} />
      {:else}<Shield size={15} strokeWidth={2} />{/if}
    </span>
    <span class="titles">
      <span class="kind">{kindLabel}</span>
      <span class="title">{heading}</span>
    </span>
    {#if secondsLeft !== null}
      <span class="timeout" aria-label="Time remaining">{secondsLeft}s</span>
    {/if}
  </div>

  {#if titleBody}
    <p class="message title-body">{titleBody}</p>
  {/if}
  {#if request.message}
    <p class="message">{request.message}</p>
  {/if}

  {#if request.method === 'open_url' && request.url}
    <div class="url-row">
      <code class="url">{request.url}</code>
    </div>
    {#if request.launchUrl && request.launchUrl !== request.url}
      <p class="launch-note">Open uses the destination shown above. Copy preserves the harness sign-in shortcut.</p>
    {/if}
  {/if}
  {#if linkFeedback}<p class="link-feedback" role="status">{linkFeedback}</p>{/if}

  {#if request.cwd || request.toolName || scalarArgs.length > 0}
    <div class="kvs">
      {#if request.toolName}
        <div class="kv"><span class="k">Tool</span><code class="v">{request.toolName}</code></div>
      {/if}
      {#each scalarArgs as [key, value], argIndex (argIndex + ':' + key)}
        <div class="kv"><span class="k">{key}</span><code class="v">{String(value)}</code></div>
      {/each}
      {#if request.cwd}
        <div class="kv"><span class="k">Directory</span><code class="v">{request.cwd}</code></div>
      {/if}
    </div>
  {/if}
  {#if complexArgs > 0}
    <details class="raw">
      <summary>Arguments</summary>
      <pre>{prettyJson(request.toolArgs)}</pre>
    </details>
  {/if}

  <fieldset class="actions" disabled={answered}>
    {#if request.method === 'select' && request.options && request.options.length > 0}
      <div class="options">
        {#each request.options as option, i (i + ':' + option)}
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
          onkeydown={(e) => {
            if (e.isComposing || e.keyCode === 229) return;
            if (e.key === 'Enter') void respond({ value: text });
          }}
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
            if (answered || responding) return;
            responding = true;
            const requestId = request.id;
            try {
              const opened = await openExternal(request.url ?? '');
              if (request.id !== requestId) return;
              if (opened) {
                answered = true;
                try { await onRespond(requestId, { confirmed: true }); }
                catch { if (request.id === requestId) answered = false; }
              } else {
                linkFeedback = 'Could not open the destination. Copy the sign-in shortcut or use the URL above.';
              }
            } finally {
              if (request.id === requestId) responding = false;
            }
          }}
        >
          Open destination
        </button>
        <button type="button" class="btn ghost" onclick={async () => {
          const requestId = request.id;
          try {
            await navigator.clipboard.writeText(request.launchUrl ?? request.url ?? '');
            if (request.id === requestId) linkFeedback = 'Sign-in shortcut copied. Open it in your browser to continue.';
          } catch {
            if (request.id === requestId) linkFeedback = 'Clipboard unavailable; select and copy the URL above.';
          }
        }}>Copy sign-in shortcut</button>
        <button type="button" class="btn ghost" onclick={() => respond({ cancelled: true })}>Dismiss</button>
      </div>
    {:else if request.options && request.options.length > 0}
      <div class="row-actions">
        {#each request.options as option, i (i + ':' + option)}
          {@const last = i === request.options.length - 1}
          {@const permission = request.method === 'permission'}
          <button
            type="button"
            class="btn"
            class:primary={!permission && last}
            class:ghost={!permission && !last}
            class:danger={permission && /\b(deny|reject|block|no)\b/i.test(option)}
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
    margin: 0 0 10px;
    padding: 14px 16px;
    border-radius: var(--radius-xl);
    background: var(--elevated);
    box-shadow: var(--shadow);
    animation: ui-rise 0.18s var(--ease);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 11px;
  }
  .icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex: none;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    color: var(--warn);
    background: var(--warn-bg);
  }
  .titles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .kind {
    color: var(--warn);
    font-size: 11px;
    font-weight: 600;
  }
  .title {
    font-weight: 600;
    font-size: 14px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .timeout {
    flex: none;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--muted);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .message {
    margin: 10px 0 0;
    font-size: 13px;
    color: var(--muted);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .title-body {
    max-height: 240px;
    overflow: auto;
    color: var(--text);
  }
  .launch-note {
    margin: 6px 0 0;
    color: var(--muted);
    font-size: 11.5px;
  }
  .url-row {
    margin-top: 10px;
    padding: 7px 10px;
    border-radius: var(--radius);
    background: var(--surface);
    overflow-x: auto;
  }
  .url {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--accent);
    white-space: nowrap;
  }
  .link-feedback {
    margin: 8px 0 0;
    color: var(--warn);
    font-size: 12px;
  }
  .kvs {
    margin-top: 12px;
    padding: 8px 12px;
    border-radius: var(--radius);
    background: var(--surface);
    border: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .kv {
    display: flex;
    gap: 12px;
    align-items: baseline;
    font-size: 12px;
    min-width: 0;
  }
  .k {
    flex: none;
    min-width: 72px;
    color: var(--subtle);
  }
  .v {
    font-family: var(--mono);
    font-size: 12px;
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .raw {
    margin-top: 8px;
  }
  .raw summary {
    cursor: pointer;
    color: var(--muted);
    font-size: 12px;
  }
  .raw pre {
    margin: 6px 0 0;
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--surface);
    border: 1px solid var(--line);
    font-family: var(--mono);
    font-size: 11.5px;
    overflow-x: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 240px;
    overflow-y: auto;
  }
  .actions {
    margin: 14px 0 0;
    border: 0;
    padding: 0;
    min-width: 0;
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 10px;
  }
  .opt {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 12px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text);
    font-size: 13px;
    text-align: left;
    transition: border-color 0.12s, background 0.12s;
  }
  .opt:hover {
    border-color: var(--accent);
    background: var(--accent-bg);
  }
  .opt-desc {
    color: var(--muted);
    font-size: 12px;
  }
  .row-actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    flex-wrap: wrap;
  }
  .btn {
    height: 30px;
    padding: 0 14px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--surface);
    color: var(--text);
    font-size: 12.5px;
    font-weight: 500;
    transition: background 0.12s, filter 0.12s;
  }
  .btn:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .btn.primary {
    background: var(--accent-strong);
    border-color: transparent;
    color: var(--on-accent);
    font-weight: 600;
  }
  .btn.primary:hover:not(:disabled) {
    background: var(--accent-strong);
    filter: brightness(1.08);
  }
  .btn.danger {
    color: var(--bad);
  }
  .btn.ghost {
    background: transparent;
    border-color: transparent;
    color: var(--muted);
  }
  .btn.ghost:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-2);
  }
  .field {
    width: 100%;
    padding: 8px 10px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: 13px;
    font-family: inherit;
    resize: vertical;
    margin-bottom: 10px;
  }
  .field:focus {
    border-color: var(--accent);
    box-shadow: var(--focus-ring);
  }
</style>
