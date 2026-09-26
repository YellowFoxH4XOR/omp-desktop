<script lang="ts">
  import { onMount } from 'svelte';
  import { Braces, Download, Globe, KeyRound, LoaderCircle, Pencil, Plug, Plus, Terminal, Trash2 } from '@lucide/svelte';
  import { api } from '$lib/api';
  import type { McpImportSource, McpOverview, McpServer } from '$lib/types';

  /** Opens πDesk's terminal and types `command` into it. */
  let { onRunInTerminal }: { onRunInTerminal?: (command: string) => void } = $props();

  interface Draft {
    original: string | null;
    name: string;
    kind: 'stdio' | 'http';
    command: string;
    args: string;
    env: string;
    url: string;
    headers: string;
    auth: '' | 'bearer' | 'oauth';
    lifecycle: string;
    /** Fields the form doesn't edit, kept as they were. */
    base: Record<string, unknown>;
  }

  let overview = $state<McpOverview | null>(null);
  let loading = $state(true);
  let busy = $state(false);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let draft = $state<Draft | null>(null);
  let formError = $state('');
  let confirmRemove = $state<string | null>(null);
  let rawOpen = $state(false);
  let rawText = $state('');
  let rawError = $state('');
  let importing = $state<McpImportSource | null>(null);
  let picked = $state<Set<string>>(new Set());

  const existing = $derived(new Set(overview?.servers.map(server => server.name) ?? []));
  const needsSignIn = $derived(overview?.servers.filter(server => server.auth === 'oauth' && !server.disabled) ?? []);

  function errorText(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }
  async function load() {
    try { overview = await api.mcpOverview(); }
    catch (error) { notice = { kind: 'error', text: errorText(error) }; }
    finally { loading = false; }
  }
  async function act(run: () => Promise<unknown>, done?: string) {
    if (busy) return false;
    busy = true;
    notice = null;
    try {
      await run();
      await load();
      if (done) notice = { kind: 'ok', text: `${done} New threads use it; restart running threads to pick it up.` };
      return true;
    } catch (error) {
      notice = { kind: 'error', text: errorText(error) };
      return false;
    } finally {
      busy = false;
    }
  }

  function lines(value: unknown, join: string): string {
    if (!value || typeof value !== 'object') return '';
    return Object.entries(value as Record<string, unknown>).map(([key, val]) => `${key}${join}${String(val)}`).join('\n');
  }
  function blankDraft(): Draft {
    return { original: null, name: '', kind: 'http', command: '', args: '', env: '', url: '', headers: '', auth: '', lifecycle: '', base: {} };
  }
  function edit(server: McpServer) {
    const config = { ...(server.config ?? {}) };
    draft = {
      original: server.name,
      name: server.name,
      kind: typeof config.command === 'string' ? 'stdio' : 'http',
      command: typeof config.command === 'string' ? config.command : '',
      args: Array.isArray(config.args) ? config.args.map(String).join('\n') : '',
      env: lines(config.env, '='),
      url: typeof config.url === 'string' ? config.url : '',
      headers: lines(config.headers, ': '),
      auth: config.auth === 'bearer' || config.auth === 'oauth' ? config.auth : '',
      lifecycle: typeof config.lifecycle === 'string' ? config.lifecycle : '',
      base: config,
    };
    formError = '';
  }
  function parsePairs(text: string, separator: string, what: string): Record<string, string> | undefined {
    const pairs: Record<string, string> = {};
    for (const line of text.split('\n').map(line => line.trim()).filter(Boolean)) {
      const at = line.indexOf(separator);
      if (at <= 0) throw new Error(`Write each ${what} as NAME${separator === '=' ? '=' : ': '}value.`);
      pairs[line.slice(0, at).trim()] = line.slice(at + 1).trim();
    }
    return Object.keys(pairs).length ? pairs : undefined;
  }
  function toConfig(value: Draft): Record<string, unknown> {
    const config: Record<string, unknown> = { ...value.base };
    for (const key of ['command', 'args', 'env', 'url', 'headers', 'socket', 'auth', 'lifecycle']) delete config[key];
    if (value.kind === 'stdio') {
      if (!value.command.trim()) throw new Error('Enter the command that starts the server, for example npx.');
      config.command = value.command.trim();
      const args = value.args.split('\n').map(arg => arg.trim()).filter(Boolean);
      if (args.length) config.args = args;
      const env = parsePairs(value.env, '=', 'environment variable');
      if (env) config.env = env;
    } else {
      if (!value.url.trim()) throw new Error('Enter the server URL, for example https://mcp.example.com/mcp.');
      config.url = value.url.trim();
      const headers = parsePairs(value.headers, ':', 'header');
      if (headers) config.headers = headers;
    }
    if (value.auth) config.auth = value.auth;
    if (value.lifecycle) config.lifecycle = value.lifecycle;
    return config;
  }
  async function saveDraft(event: SubmitEvent) {
    event.preventDefault();
    if (!draft) return;
    let config: Record<string, unknown>;
    try { config = toConfig(draft); formError = ''; }
    catch (error) { formError = errorText(error); return; }
    const current = draft;
    const saved = await act(() => api.mcpSaveServer(current.original, current.name.trim(), config), `${current.original ? 'Saved' : 'Added'} ${current.name.trim()}.`);
    if (saved) draft = null;
    else if (notice?.kind === 'error') { formError = notice.text; notice = null; }
  }

  function openRaw() {
    rawText = overview?.raw || '{\n  "mcpServers": {}\n}\n';
    rawError = '';
    rawOpen = true;
  }
  async function saveRaw() {
    rawError = '';
    const saved = await act(() => api.mcpSaveRaw(rawText), 'Saved mcp.json.');
    if (saved) rawOpen = false;
    else if (notice?.kind === 'error') { rawError = notice.text; notice = null; }
  }

  function startImport(source: McpImportSource) {
    importing = source;
    picked = new Set(source.servers.filter(server => !existing.has(server.name)).map(server => server.name));
  }
  function toggle(name: string) {
    const next = new Set(picked);
    if (next.has(name)) next.delete(name); else next.add(name);
    picked = next;
  }
  async function runImport() {
    if (!importing) return;
    const source = importing;
    let copied: string[] = [];
    const ok = await act(async () => { copied = await api.mcpImport(source.id, [...picked]); });
    if (ok) {
      importing = null;
      notice = { kind: 'ok', text: copied.length ? `Copied ${copied.join(', ')} into πDesk. New threads use them; restart running threads to pick them up.` : 'Nothing new to copy.' };
    }
  }

  onMount(() => { void load(); });
</script>

<div class="mcp">
  {#if loading}
    <div class="empty"><LoaderCircle size={16} class="spin" />Loading MCP servers…</div>
  {:else if overview}
    {#if !overview.adapterInstalled}
      <div class="callout">
        <Plug size={18} />
        <div><strong>MCP support isn't installed</strong><p>πDesk uses the Pi MCP adapter extension to connect MCP servers.</p></div>
        <button class="primary" disabled={busy} onclick={() => void act(() => api.mcpInstallAdapter(), 'Installed the Pi MCP adapter.')}>{busy ? 'Installing…' : 'Install adapter'}</button>
      </div>
    {/if}

    {#if notice}<p class="notice" class:error={notice.kind === 'error'} role={notice.kind === 'error' ? 'alert' : 'status'}>{notice.text}</p>{/if}
    {#if overview.error}<p class="notice error" role="alert">{overview.error} Fix it in <button class="link" onclick={openRaw}>mcp.json</button>.</p>{/if}

    {#each overview.importSources as source (source.id)}
      {@const fresh = source.servers.filter(server => !existing.has(server.name))}
      {#if fresh.length && importing?.id !== source.id}
        <div class="callout import">
          <Download size={18} />
          <div><strong>{fresh.length} server{fresh.length === 1 ? '' : 's'} in {source.path}</strong><p>That file belongs to your other tools, so πDesk doesn't load it. Copy the ones you want: {fresh.map(server => server.name).join(', ')}.</p></div>
          <button class="secondary" onclick={() => startImport(source)}>Choose servers…</button>
        </div>
      {/if}
    {/each}

    {#if importing}
      <div class="panel" role="group" aria-label="Copy MCP servers">
        <div class="panel-head"><strong>Copy from {importing.path}</strong><span class="muted">Copied servers become πDesk's own; the original file isn't changed.</span></div>
        <ul class="pick">
          {#each importing.servers as server (server.name)}
            {@const already = existing.has(server.name)}
            <li>
              <label class:disabled={already}>
                <input type="checkbox" checked={picked.has(server.name)} disabled={already} onchange={() => toggle(server.name)} />
                <span class="pick-name">{server.name}</span>
                <span class="chip">{server.transport === 'stdio' ? 'Command' : 'URL'}</span>
                {#if server.auth === 'oauth'}<span class="chip">Sign-in</span>{/if}
                {#if server.hasSecrets}<span class="chip warn" title="Its keys or tokens are copied too">Includes keys</span>{/if}
                {#if already}<span class="muted small">already added</span>{/if}
              </label>
              <code>{server.target}</code>
            </li>
          {/each}
        </ul>
        <div class="row-actions">
          <button class="ghost" onclick={() => (importing = null)}>Cancel</button>
          <button class="primary" disabled={busy || picked.size === 0} onclick={() => void runImport()}>Copy {picked.size} server{picked.size === 1 ? '' : 's'}</button>
        </div>
      </div>
    {/if}

    <div class="bar">
      <span class="muted">{overview.servers.length ? `${overview.servers.length} server${overview.servers.length === 1 ? '' : 's'} · ${overview.path}` : `No servers yet · ${overview.path}`}</span>
      <span class="grow"></span>
      <button class="secondary" onclick={openRaw}><Braces size={13} />Edit mcp.json</button>
      <button class="primary" onclick={() => { draft = blankDraft(); formError = ''; }}><Plus size={14} />Add server</button>
    </div>

    {#if draft && !draft.original}{@render form()}{/if}

    {#if overview.servers.length}
      <ul class="servers" aria-label="MCP servers">
        {#each overview.servers as server (server.name)}
          <li class:off={server.disabled}>
            {#if draft?.original === server.name}
              {@render form()}
            {:else}
              <span class="kind" aria-hidden="true">{#if server.transport === 'stdio'}<Terminal size={15} />{:else}<Globe size={15} />{/if}</span>
              <div class="info">
                <div class="title-row">
                  <strong>{server.name}</strong>
                  {#if server.disabled}<span class="chip">Off</span>{/if}
                  {#if server.auth === 'oauth'}<span class="chip">Sign-in</span>{:else if server.auth === 'bearer'}<span class="chip">Token</span>{/if}
                  {#if server.hasSecrets}<span class="chip" title="Has environment values, headers, or a token"><KeyRound size={11} />keys</span>{/if}
                  {#if server.lifecycle && server.lifecycle !== 'lazy'}<span class="chip">{server.lifecycle}</span>{/if}
                </div>
                <code title={server.target}>{server.target}</code>
              </div>
              <div class="actions">
                {#if confirmRemove === server.name}
                  <span class="muted small">Remove {server.name}?</span>
                  <button class="ghost" onclick={() => (confirmRemove = null)}>Cancel</button>
                  <button class="danger" disabled={busy} onclick={() => void act(() => api.mcpRemoveServer(server.name), `Removed ${server.name}.`).then(() => (confirmRemove = null))}>Remove</button>
                {:else}
                  <label class="switch" title={server.disabled ? 'Turn on' : 'Turn off'}>
                    <input type="checkbox" role="switch" aria-label={`${server.name} enabled`} checked={!server.disabled} disabled={busy}
                      onchange={event => void act(() => api.mcpSetEnabled(server.name, event.currentTarget.checked), `${event.currentTarget.checked ? 'Turned on' : 'Turned off'} ${server.name}.`)} />
                    <span></span>
                  </label>
                  <button class="icon" title="Edit" aria-label={`Edit ${server.name}`} onclick={() => edit(server)}><Pencil size={14} /></button>
                  <button class="icon" title="Remove" aria-label={`Remove ${server.name}`} onclick={() => (confirmRemove = server.name)}><Trash2 size={14} /></button>
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if !draft && !importing}
      <div class="empty column">
        <Plug size={22} strokeWidth={1.6} />
        <strong>No MCP servers yet</strong>
        <span>Add a server by its URL or command. Threads reach MCP tools through one <code>mcp</code> tool, and servers connect only when used.</span>
      </div>
    {/if}

    {#if needsSignIn.length}
      <div class="callout signin">
        <KeyRound size={18} />
        <div>
          <strong>{needsSignIn.length === 1 ? `${needsSignIn[0].name} needs` : 'These servers need'} a one-time sign-in</strong>
          <p>Sign in opens πDesk's terminal and runs <code>pi '/mcp-auth {needsSignIn.length === 1 ? needsSignIn[0].name : '<server>'}'</code>, which opens your browser. The token is kept in your macOS keychain.</p>
          {#if onRunInTerminal}
            <div class="signin-list">
              {#each needsSignIn as server (server.name)}
                <button class="secondary" aria-label={`Sign in to ${server.name}`} onclick={() => onRunInTerminal(`pi '/mcp-auth ${server.name}'`)}><Terminal size={13} />{needsSignIn.length === 1 ? 'Sign in' : `Sign in to ${server.name}`}</button>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}

    <h3 class="sub">Options</h3>
    <div class="option">
      <div><strong>Ask before every MCP tool call</strong><p>{overview.approveTools === 'custom' ? 'mcp.json has custom approval rules; edit them there.' : 'Threads show a permission card before running any MCP tool, even in Auto.'}</p></div>
      {#if overview.approveTools !== 'custom'}
        <label class="switch">
          <input type="checkbox" role="switch" aria-label="Ask before every MCP tool call" checked={overview.approveTools === 'all'} disabled={busy}
            onchange={event => void act(() => api.mcpSetApproveTools(event.currentTarget.checked))} />
          <span></span>
        </label>
      {/if}
    </div>

    {#if rawOpen}
      <div class="panel">
        <div class="panel-head"><strong>{overview.path}</strong><span class="muted">Every adapter option lives here. Comments are kept when you save this editor; the form above rewrites the file without them.</span></div>
        <textarea class="raw" bind:value={rawText} spellcheck="false" aria-label="mcp.json"></textarea>
        {#if rawError}<p class="field-error" role="alert">{rawError}</p>{/if}
        <div class="row-actions">
          <button class="ghost" onclick={() => (rawOpen = false)}>Cancel</button>
          <button class="primary" disabled={busy} onclick={() => void saveRaw()}>Save mcp.json</button>
        </div>
      </div>
    {/if}
    <p class="hint">Adapter {overview.adapterVersion ? `v${overview.adapterVersion}` : 'not installed'}. {#if overview.hasComments}mcp.json has comments; editing a server here removes them.{/if}</p>
  {/if}
</div>

{#snippet form()}
  {#if draft}
    <form class="panel form" onsubmit={saveDraft} aria-label={draft.original ? `Edit ${draft.original}` : 'Add MCP server'}>
      <div class="grid2">
        <label>Name<input bind:value={draft.name} placeholder="context7" maxlength="64" spellcheck="false" required /></label>
        <div class="field">Connect with
          <div class="seg" role="radiogroup" aria-label="Server type">
            <button type="button" role="radio" aria-checked={draft.kind === 'http'} class:on={draft.kind === 'http'} onclick={() => draft && (draft.kind = 'http')}><Globe size={13} />URL</button>
            <button type="button" role="radio" aria-checked={draft.kind === 'stdio'} class:on={draft.kind === 'stdio'} onclick={() => draft && (draft.kind = 'stdio')}><Terminal size={13} />Command</button>
          </div>
        </div>
      </div>
      {#if draft.kind === 'http'}
        <label>URL<input bind:value={draft.url} placeholder="https://mcp.context7.com/mcp" spellcheck="false" /></label>
        <label>Headers <span class="muted">one per line · Name: value · use ${'{'}VAR{'}'} to read an environment variable</span><textarea bind:value={draft.headers} rows="2" spellcheck="false" placeholder="Authorization: Bearer ${'{'}CONTEXT7_TOKEN{'}'}"></textarea></label>
      {:else}
        <label>Command<input bind:value={draft.command} placeholder="npx" spellcheck="false" /></label>
        <label>Arguments <span class="muted">one per line</span><textarea bind:value={draft.args} rows="2" spellcheck="false" placeholder={'-y\nchrome-devtools-mcp@latest'}></textarea></label>
        <label>Environment <span class="muted">one per line · NAME=value</span><textarea bind:value={draft.env} rows="2" spellcheck="false" placeholder="API_KEY=…"></textarea></label>
      {/if}
      <div class="grid2">
        <label>Sign-in
          <select bind:value={draft.auth}><option value="">None</option><option value="oauth">OAuth (browser sign-in)</option><option value="bearer">Bearer token</option></select>
        </label>
        <label>Connect
          <select bind:value={draft.lifecycle}><option value="">When first used (default)</option><option value="eager">At thread start</option><option value="keep-alive">At start, stay connected</option><option value="lazy-keep-alive">When first used, stay connected</option></select>
        </label>
      </div>
      {#if formError}<p class="field-error" role="alert">{formError}</p>{/if}
      <div class="row-actions">
        <button type="button" class="ghost" onclick={() => (draft = null)}>Cancel</button>
        <button type="submit" class="primary" disabled={busy || !draft.name.trim()}>{draft.original ? 'Save' : 'Add server'}</button>
      </div>
    </form>
  {/if}
{/snippet}

<style>
  .mcp { display: flex; flex-direction: column; gap: 14px; }
  .muted { color: var(--muted); font-size: 12.5px; }
  .small { font-size: 12px; }
  .grow { flex: 1; }
  :global(.spin) { animation: mcp-spin 0.9s linear infinite; }
  @keyframes mcp-spin { to { transform: rotate(360deg); } }
  @keyframes mcp-in { from { opacity: 0; transform: translateY(-4px); } }
  code { font-family: var(--mono); font-size: 11.5px; }

  button { display: inline-flex; align-items: center; justify-content: center; gap: 6px; height: 30px; padding: 0 12px; border-radius: var(--radius); font: 600 12.5px var(--font); white-space: nowrap; border: 1px solid transparent; }
  button:disabled { opacity: 0.5; }
  button:focus-visible, input:focus-visible, select:focus-visible, textarea:focus-visible { outline: none; box-shadow: var(--focus-ring); }
  .primary { background: var(--accent-strong); color: var(--on-accent); box-shadow: var(--shadow-sm); }
  .primary:hover:not(:disabled) { filter: brightness(1.08); }
  .secondary { background: var(--surface); border-color: var(--line-strong); color: var(--text); }
  .secondary:hover:not(:disabled) { background: var(--surface-2); }
  .ghost { background: none; color: var(--muted); }
  .ghost:hover { color: var(--text); background: var(--surface-2); }
  .danger { background: var(--bad-bg); color: var(--bad); }
  .icon { width: 28px; padding: 0; background: none; color: var(--muted); }
  .icon:hover:not(:disabled) { background: var(--surface-2); color: var(--text); }
  .link { height: auto; padding: 0; border: 0; background: none; color: var(--accent); font: inherit; }

  .notice { margin: 0; padding: 10px 12px; border-radius: var(--radius); background: var(--good-bg); color: var(--text); font-size: 12.5px; animation: mcp-in 0.25s var(--ease); }
  .notice.error { background: var(--bad-bg); color: var(--bad); }
  .callout { display: flex; align-items: center; gap: 12px; padding: 14px 16px; border: 1px solid var(--line-strong); border-radius: 12px; background: var(--panel); animation: mcp-in 0.25s var(--ease); }
  .callout > :global(svg) { flex: none; color: var(--accent); }
  .callout.import { border-color: color-mix(in srgb, var(--accent) 35%, var(--line-strong)); background: color-mix(in srgb, var(--accent-bg) 55%, var(--panel)); }
  .callout div { flex: 1; min-width: 0; }
  .callout.signin { align-items: flex-start; }
  .signin-list { display: flex; flex-wrap: wrap; gap: 8px; margin-top: 10px; }
  .callout strong { font-size: 13px; }
  .callout p { margin: 2px 0 0; color: var(--muted); font-size: 12.5px; line-height: 1.5; }

  .bar { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .empty { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 120px; padding: 24px; border: 1px dashed var(--line-strong); border-radius: 12px; color: var(--muted); font-size: 13px; text-align: center; }
  .empty.column { flex-direction: column; }
  .empty strong { color: var(--text); }
  .empty span { max-width: 52ch; line-height: 1.5; }

  .servers { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 12px; background: var(--bg); overflow: hidden; }
  .servers li { display: flex; align-items: center; gap: 12px; padding: 12px 14px; animation: mcp-in 0.25s var(--ease); }
  .servers li + li { border-top: 1px solid var(--line); }
  .servers li.off .info, .servers li.off .kind { opacity: 0.55; }
  .kind { flex: none; width: 30px; height: 30px; display: grid; place-items: center; border-radius: 8px; background: var(--accent-bg); color: var(--accent); }
  .info { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 3px; }
  .title-row { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; font-size: 13.5px; }
  .info code { color: var(--subtle); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chip { display: inline-flex; align-items: center; gap: 3px; height: 18px; padding: 0 6px; border-radius: 5px; background: var(--surface-2); color: var(--muted); font-size: 11px; font-weight: 500; }
  .chip.warn { background: var(--warn-bg); color: var(--warn); }
  .actions { flex: none; display: flex; align-items: center; gap: 4px; }

  .switch { position: relative; display: inline-flex; width: 34px; height: 20px; margin-right: 4px; cursor: pointer; }
  .switch input { position: absolute; inset: 0; z-index: 1; opacity: 0; margin: 0; cursor: pointer; }
  .switch span { flex: 1; pointer-events: none; border-radius: 999px; background: var(--surface-2); box-shadow: inset 0 0 0 1px var(--line-strong); transition: background 0.18s; }
  .switch span::after { content: ''; position: absolute; top: 3px; left: 3px; width: 14px; height: 14px; border-radius: 50%; background: var(--bg); box-shadow: var(--shadow-sm); transition: transform 0.2s var(--ease); }
  .switch input:checked + span { background: var(--accent-strong); box-shadow: none; }
  .switch input:checked + span::after { transform: translateX(14px); }
  .switch input:focus-visible + span { box-shadow: var(--focus-ring); }

  .panel { display: flex; flex-direction: column; gap: 12px; padding: 16px; border: 1px solid var(--line-strong); border-radius: 12px; background: var(--panel); animation: mcp-in 0.25s var(--ease); }
  .servers li .panel { flex: 1; border: 0; padding: 4px 0; background: none; }
  .panel-head { display: flex; flex-direction: column; gap: 2px; }
  .panel-head strong { font-size: 13px; }
  .pick { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .pick li { display: flex; flex-direction: column; gap: 2px; }
  .pick label { display: flex; align-items: center; gap: 8px; font-size: 13px; cursor: pointer; }
  .pick label.disabled { cursor: default; opacity: 0.6; }
  .pick-name { font-weight: 600; }
  .pick code { margin-left: 24px; color: var(--subtle); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .row-actions { display: flex; justify-content: flex-end; gap: 8px; }

  .form label, .form .field { display: flex; flex-direction: column; gap: 5px; font-size: 12px; font-weight: 600; color: var(--text); }
  .form label .muted { font-weight: 400; font-size: 11.5px; }
  .grid2 { display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 12px; }
  input:not([type='checkbox']), select, textarea { height: 32px; padding: 0 10px; border: 1px solid var(--line-strong); border-radius: 8px; background: var(--bg); color: var(--text); font: 400 13px var(--font); }
  textarea { height: auto; padding: 8px 10px; resize: vertical; font: 12.5px/1.5 var(--mono); }
  input[spellcheck='false'] { font-family: var(--mono); font-size: 12.5px; }
  .seg { display: inline-flex; align-self: flex-start; padding: 2px; border-radius: 9px; background: var(--surface-2); }
  .seg button { height: 26px; padding: 0 10px; border: 0; border-radius: 7px; background: none; color: var(--muted); }
  .seg button.on { background: var(--bg); color: var(--text); box-shadow: var(--shadow-sm); }
  .field-error { margin: 0; color: var(--bad); font-size: 12.5px; }

  .sub { margin: 10px 0 0; font-size: 12px; font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase; color: var(--subtle); }
  .option { display: flex; align-items: center; gap: 16px; padding: 12px 16px; border: 1px solid var(--line); border-radius: 12px; background: var(--panel); }
  .option div { flex: 1; }
  .option strong { font-size: 13px; }
  .option p { margin: 2px 0 0; color: var(--muted); font-size: 12.5px; }
  .raw { min-height: 260px; }
  .hint { margin: 0; color: var(--subtle); font-size: 12px; }
</style>
