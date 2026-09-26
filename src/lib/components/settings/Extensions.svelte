<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowUpRight, Check, Download, LoaderCircle, Package, RefreshCw, Search, Trash2 } from '@lucide/svelte';
  import { api } from '$lib/api';
  import { ago } from '$lib/time';
  import { openExternal } from '../conversation/links';
  import { checkForUpdates, isFresh, uncheckedCount, updateCheck } from '$lib/extension-updates.svelte';
  import type { CatalogPackage, InstalledPackage } from '$lib/types';

  let { onUpdates }: { onUpdates?: (count: number) => void } = $props();

  let tab = $state<'installed' | 'discover'>('installed');
  let installed = $state<InstalledPackage[]>([]);
  let loadingInstalled = $state(true);
  const checking = $derived(updateCheck.running);
  const checkedAt = $derived(updateCheck.at);
  let busy = $state<Record<string, 'install' | 'update' | 'remove'>>({});
  let confirmRemove = $state<string | null>(null);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let sourceInput = $state('');

  let query = $state('');
  let kind = $state('');
  let sort = $state('downloads');
  let results = $state<CatalogPackage[]>([]);
  let page = $state(1);
  let hasMore = $state(false);
  let searching = $state(false);
  let searchError = $state('');
  let searchToken = 0;

  const updates = $derived(installed.filter(item => item.updateAvailable));
  const unchecked = $derived(uncheckedCount(installed));
  const installedNames = $derived(new Set(installed.map(item => item.name)));

  function errorText(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }
  function npmName(source: string): string {
    const spec = source.replace(/^npm:/, '');
    const at = spec.indexOf('@', 1);
    return at > 0 ? spec.slice(0, at) : spec;
  }
  /** Carry the last check's versions onto a freshly read installed list. */
  function withCheck(list: InstalledPackage[]): InstalledPackage[] {
    const known = new Map(updateCheck.packages.map(item => [item.source, item]));
    return list.map(item => {
      const checked = known.get(item.source);
      return checked && checked.version === item.version ? { ...item, latest: checked.latest, updateAvailable: checked.updateAvailable } : item;
    });
  }

  async function loadInstalled() {
    try { installed = withCheck(await api.extensionsInstalled()); }
    catch (error) { notice = { kind: 'error', text: errorText(error) }; }
    finally { loadingInstalled = false; }
  }
  async function checkUpdates() {
    if (checking) return;
    try {
      installed = await checkForUpdates();
      onUpdates?.(installed.filter(item => item.updateAvailable).length);
    } catch (error) { notice = { kind: 'error', text: errorText(error) }; }
  }

  async function change(source: string, action: 'install' | 'update' | 'remove') {
    if (busy[source]) return;
    busy[source] = action;
    notice = null;
    try {
      let label = source;
      if (action === 'install') label = await api.extensionsInstall(source);
      else if (action === 'update') await api.extensionsUpdate(source);
      else await api.extensionsRemove(source);
      await checkUpdates();
      const verb = action === 'install' ? 'Installed' : action === 'update' ? 'Updated' : 'Removed';
      notice = { kind: 'ok', text: `${verb} ${label.replace(/^npm:/, '')}. New threads use it right away; restart running threads to pick it up.` };
      if (action === 'install') sourceInput = '';
    } catch (error) {
      notice = { kind: 'error', text: errorText(error) };
    } finally {
      delete busy[source];
      if (confirmRemove === source) confirmRemove = null;
    }
  }
  async function updateAll() {
    for (const item of updates) await change(item.source, 'update');
  }
  function installTyped(event: SubmitEvent) {
    event.preventDefault();
    const value = sourceInput.trim();
    if (value) void change(value, 'install');
  }

  async function search(reset: boolean) {
    const token = ++searchToken;
    const nextPage = reset ? 1 : page + 1;
    searching = true;
    searchError = '';
    try {
      const result = await api.extensionsCatalog(query.trim(), kind, sort, nextPage);
      if (token !== searchToken) return;
      const seen = new Set(reset ? [] : results.map(item => item.name));
      results = [...(reset ? [] : results), ...result.packages.filter(item => !seen.has(item.name))];
      page = result.page;
      hasMore = result.hasMore;
    } catch (error) {
      if (token === searchToken) searchError = errorText(error);
    } finally {
      if (token === searchToken) searching = false;
    }
  }

  // Search as you type, once the user stops for a moment.
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  let searchedOnce = false;
  $effect(() => {
    const signature = `${query}|${kind}|${sort}|${tab}`;
    if (tab !== 'discover') return;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void search(true), searchedOnce ? 350 : 0);
    searchedOnce = true;
    return () => { void signature; clearTimeout(searchTimer); };
  });

  onMount(() => {
    void loadInstalled().then(() => {
      if (!isFresh()) void checkUpdates();
      else onUpdates?.(installed.filter(item => item.updateAvailable).length);
    });
  });
</script>

<div class="ext">
  <form class="install" onsubmit={installTyped}>
    <label for="ext-source">Install a package</label>
    <div class="field">
      <span class="prompt">$ pi install</span>
      <input id="ext-source" bind:value={sourceInput} placeholder="npm:pi-mcp-adapter  or  git:github.com/owner/repo" autocomplete="off" spellcheck="false" maxlength="300" />
      <button class="primary" type="submit" disabled={!sourceInput.trim() || Object.values(busy).includes('install')}>
        {#if Object.values(busy).includes('install')}<LoaderCircle size={14} class="spin" />Installing…{:else}<Download size={14} />Install{/if}
      </button>
    </div>
    <p class="hint">Installs into πDesk's private Pi only. Packages run code in every Pi thread with your permissions, so install ones you trust.</p>
  </form>

  {#if notice}<p class="notice" class:error={notice.kind === 'error'} role={notice.kind === 'error' ? 'alert' : 'status'}>{notice.text}</p>{/if}

  <div class="tabs" role="tablist" aria-label="Extensions">
    <button role="tab" aria-selected={tab === 'installed'} class:on={tab === 'installed'} onclick={() => (tab = 'installed')}>
      Installed <span class="count">{installed.length}</span>{#if updates.length}<span class="dot" title={`${updates.length} update${updates.length === 1 ? '' : 's'} available`}></span>{/if}
    </button>
    <button role="tab" aria-selected={tab === 'discover'} class:on={tab === 'discover'} onclick={() => (tab = 'discover')}>Discover</button>
  </div>

  {#if tab === 'installed'}
    <div class="bar">
      <span class="muted">
        {#if checking}Checking for updates…
        {:else if updates.length}{updates.length} update{updates.length === 1 ? '' : 's'} available
        {:else if checkedAt && unchecked}Couldn't reach npm for {unchecked} package{unchecked === 1 ? '' : 's'} · <button class="link" onclick={() => void checkUpdates()}>Try again</button>
        {:else if checkedAt}Up to date · checked {ago(new Date(checkedAt).toISOString())}
        {:else}Not checked yet{/if}
      </span>
      <span class="grow"></span>
      {#if updates.length > 1}<button class="secondary" disabled={checking || Object.keys(busy).length > 0} onclick={() => void updateAll()}>Update all ({updates.length})</button>{/if}
      <button class="secondary" disabled={checking} onclick={() => void checkUpdates()}><RefreshCw size={13} class={checking ? 'spin' : ''} />Check for updates</button>
    </div>

    {#if loadingInstalled}
      <div class="empty"><LoaderCircle size={16} class="spin" />Loading packages…</div>
    {:else if installed.length === 0}
      <div class="empty column">
        <Package size={22} strokeWidth={1.6} />
        <strong>No packages yet</strong>
        <span>Install one by name above, or browse the most popular in Discover.</span>
        <button class="secondary" onclick={() => (tab = 'discover')}>Browse packages</button>
      </div>
    {:else}
      <ul class="installed" aria-label="Installed packages">
        {#each installed as item (item.source)}
          <li class:has-update={item.updateAvailable}>
            <div class="info">
              <div class="title-row">
                <strong>{item.name}</strong>
                {#if item.version}<span class="chip mono">v{item.version}</span>{/if}
                {#if item.pinned}<span class="chip" title="Pinned versions don't follow updates">pinned</span>{/if}
                {#if item.kind !== 'npm'}<span class="chip">{item.kind}</span>{/if}
              </div>
              {#if item.description}<p>{item.description}</p>{/if}
              {#if item.updateAvailable}
                <p class="update-note">Update available: <span class="mono">v{item.version}</span> → <span class="mono">v{item.latest}</span>. Please update it.</p>
              {/if}
            </div>
            <div class="actions">
              {#if confirmRemove === item.source}
                <span class="muted small">Remove {item.name}?</span>
                <button class="ghost" onclick={() => (confirmRemove = null)}>Cancel</button>
                <button class="danger" disabled={!!busy[item.source]} onclick={() => void change(item.source, 'remove')}>{busy[item.source] === 'remove' ? 'Removing…' : 'Remove'}</button>
              {:else}
                {#if item.updateAvailable}
                  <button class="primary" disabled={!!busy[item.source]} onclick={() => void change(item.source, 'update')}>
                    {#if busy[item.source] === 'update'}<LoaderCircle size={13} class="spin" />Updating…{:else}Update to v{item.latest}{/if}
                  </button>
                {:else if item.latest}
                  <span class="ok"><Check size={13} />Up to date</span>
                {:else if checkedAt && item.kind === 'npm' && !checking}
                  <span class="muted small">Couldn't check</span>
                {:else if item.kind !== 'npm'}
                  <span class="muted small" title="Only npm packages can be checked for updates">No update check</span>
                {/if}
                {#if item.kind === 'npm'}<button class="icon" title="Open on npm" aria-label={`Open ${item.name} on npm`} onclick={() => void openExternal(`https://www.npmjs.com/package/${item.name}`)}><ArrowUpRight size={14} /></button>{/if}
                <button class="icon" title="Remove" aria-label={`Remove ${item.name}`} disabled={!!busy[item.source]} onclick={() => (confirmRemove = item.source)}><Trash2 size={14} /></button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <div class="filters">
      <label class="search"><Search size={14} /><span class="sr-only">Search packages</span><input type="search" bind:value={query} placeholder="Search pi.dev packages" maxlength="100" /></label>
      <select bind:value={kind} aria-label="Package type">
        <option value="">All types</option><option value="extension">Extensions</option><option value="skill">Skills</option><option value="prompt">Prompts</option><option value="theme">Themes</option>
      </select>
      <select bind:value={sort} aria-label="Sort">
        <option value="downloads">Most downloads</option><option value="recent">Recently published</option><option value="name">A–Z</option>
      </select>
    </div>

    {#if searchError}
      <div class="empty column"><strong>Couldn't load packages</strong><span>{searchError}</span><button class="secondary" onclick={() => void search(true)}>Try again</button></div>
    {:else if results.length === 0 && searching}
      <div class="empty"><LoaderCircle size={16} class="spin" />Loading pi.dev…</div>
    {:else if results.length === 0}
      <div class="empty column"><strong>No packages match</strong><span>Try a different word or type.</span></div>
    {:else}
      <div class="grid" aria-busy={searching}>
        {#each results as item (item.name)}
          {@const isInstalled = installedNames.has(npmName(item.source))}
          <article class="card">
            <div class="card-head">
              <strong title={item.name}>{item.name}</strong>
              <button class="icon" title="Open on pi.dev" aria-label={`Open ${item.name} on pi.dev`} onclick={() => void openExternal(`https://pi.dev/packages/${item.name}`)}><ArrowUpRight size={14} /></button>
            </div>
            <p>{item.description || 'No description.'}</p>
            <div class="meta">
              {#if item.author}<span>{item.author}</span>{/if}
              {#if item.downloadsLabel}<span class="mono">{item.downloadsLabel}</span>{/if}
              {#if item.publishedMs}<span>{ago(new Date(item.publishedMs).toISOString())}</span>{/if}
              {#each item.types as type (type)}<span class="chip">{type}</span>{/each}
            </div>
            <div class="card-foot">
              <code title={`pi install ${item.source}`}>{item.source}</code>
              {#if isInstalled}
                <span class="ok"><Check size={13} />Installed</span>
              {:else}
                <button class="secondary" disabled={!!busy[item.source]} onclick={() => void change(item.source, 'install')}>
                  {#if busy[item.source]}<LoaderCircle size={13} class="spin" />Installing…{:else}<Download size={13} />Install{/if}
                </button>
              {/if}
            </div>
          </article>
        {/each}
      </div>
      {#if hasMore}<div class="more"><button class="secondary" disabled={searching} onclick={() => void search(false)}>{searching ? 'Loading…' : 'Load more'}</button></div>{/if}
    {/if}
    <p class="hint">From <button class="link" onclick={() => void openExternal('https://pi.dev/packages')}>pi.dev/packages</button>. Review a package's source before installing it.</p>
  {/if}
</div>

<style>
  .ext { display: flex; flex-direction: column; gap: 16px; }
  .mono { font-family: var(--mono); font-size: 11.5px; }
  .muted { color: var(--muted); font-size: 12.5px; }
  .small { font-size: 12px; }
  .grow { flex: 1; }
  .sr-only { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; }
  :global(.spin) { animation: ext-spin 0.9s linear infinite; }
  @keyframes ext-spin { to { transform: rotate(360deg); } }

  button { display: inline-flex; align-items: center; justify-content: center; gap: 6px; height: 30px; padding: 0 12px; border-radius: var(--radius); font: 600 12.5px var(--font); white-space: nowrap; border: 1px solid transparent; }
  button:disabled { opacity: 0.5; }
  button:focus-visible, input:focus-visible, select:focus-visible { outline: none; box-shadow: var(--focus-ring); }
  .primary { background: var(--accent-strong); color: var(--on-accent); box-shadow: var(--shadow-sm); }
  .primary:hover:not(:disabled) { filter: brightness(1.08); }
  .secondary { background: var(--surface); border-color: var(--line-strong); color: var(--text); }
  .secondary:hover:not(:disabled) { background: var(--surface-2); }
  .ghost { background: none; color: var(--muted); }
  .ghost:hover { color: var(--text); background: var(--surface-2); }
  .danger { background: var(--bad-bg); color: var(--bad); }
  .icon { width: 28px; padding: 0; background: none; color: var(--muted); }
  .icon:hover:not(:disabled) { background: var(--surface-2); color: var(--text); }
  .link { height: auto; padding: 0; border: 0; background: none; color: var(--accent); font-weight: 500; font-size: inherit; }
  .link:hover { text-decoration: underline; }

  .install { display: flex; flex-direction: column; gap: 8px; }
  .install label { font-size: 12.5px; font-weight: 600; }
  .field { display: flex; align-items: center; gap: 8px; padding: 4px 4px 4px 12px; border: 1px solid var(--line-strong); border-radius: 10px; background: var(--bg); }
  .field:focus-within { border-color: color-mix(in srgb, var(--accent) 45%, var(--line-strong)); box-shadow: var(--focus-ring); }
  .prompt { flex: none; color: var(--subtle); font: 12.5px var(--mono); }
  .field input { flex: 1; min-width: 0; height: 30px; border: 0; background: none; color: var(--text); font: 13px var(--mono); }
  .field input:focus-visible { box-shadow: none; }
  .hint { margin: 0; color: var(--subtle); font-size: 12px; line-height: 1.5; }
  .notice { margin: 0; padding: 10px 12px; border-radius: var(--radius); background: var(--good-bg); color: var(--text); font-size: 12.5px; animation: ext-in 0.25s var(--ease); }
  .notice.error { background: var(--bad-bg); color: var(--bad); }
  @keyframes ext-in { from { opacity: 0; transform: translateY(-4px); } }

  .tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--line); }
  .tabs button { position: relative; height: 34px; padding: 0 12px; border-radius: 0; background: none; color: var(--muted); font-weight: 600; }
  .tabs button:hover { color: var(--text); }
  .tabs button.on { color: var(--text); }
  .tabs button.on::after { content: ''; position: absolute; left: 8px; right: 8px; bottom: -1px; height: 2px; border-radius: 2px; background: var(--accent); }
  .count { min-width: 18px; height: 18px; padding: 0 5px; border-radius: 999px; background: var(--surface-2); color: var(--muted); font-size: 11px; line-height: 18px; font-variant-numeric: tabular-nums; }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--warn); box-shadow: 0 0 0 3px var(--warn-bg); }

  .bar { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .empty { display: flex; align-items: center; justify-content: center; gap: 8px; min-height: 120px; padding: 24px; border: 1px dashed var(--line-strong); border-radius: 12px; color: var(--muted); font-size: 13px; text-align: center; }
  .empty.column { flex-direction: column; }
  .empty strong { color: var(--text); }

  .installed { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 12px; background: var(--bg); overflow: hidden; }
  .installed li { display: flex; align-items: flex-start; gap: 16px; padding: 14px 16px; animation: ext-in 0.25s var(--ease); }
  .installed li + li { border-top: 1px solid var(--line); }
  .installed li.has-update { background: color-mix(in srgb, var(--warn-bg) 60%, transparent); }
  .info { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .title-row { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; font-size: 13.5px; }
  .info p { margin: 0; color: var(--muted); font-size: 12.5px; line-height: 1.5; }
  .info .update-note { color: var(--warn); font-weight: 600; }
  .chip { height: 18px; padding: 0 6px; border-radius: 5px; background: var(--surface-2); color: var(--muted); font-size: 11px; font-weight: 500; line-height: 18px; }
  .actions { flex: none; display: flex; align-items: center; gap: 6px; }
  .ok { display: inline-flex; align-items: center; gap: 4px; color: var(--good); font-size: 12px; font-weight: 600; }

  .filters { display: flex; gap: 8px; flex-wrap: wrap; }
  .search { flex: 1; min-width: 200px; display: flex; align-items: center; gap: 8px; height: 34px; padding: 0 10px; border: 1px solid var(--line-strong); border-radius: 10px; background: var(--bg); color: var(--subtle); }
  .search:focus-within { border-color: color-mix(in srgb, var(--accent) 45%, var(--line-strong)); box-shadow: var(--focus-ring); }
  .search input { flex: 1; min-width: 0; border: 0; background: none; color: var(--text); font: 13px var(--font); }
  .search input:focus-visible { box-shadow: none; }
  select { height: 34px; padding: 0 10px; border: 1px solid var(--line-strong); border-radius: 10px; background: var(--bg); color: var(--text); font: 12.5px var(--font); }

  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 12px; transition: opacity 0.2s; }
  .grid[aria-busy='true'] { opacity: 0.6; }
  .card { display: flex; flex-direction: column; gap: 8px; padding: 14px; border: 1px solid var(--line); border-radius: 12px; background: var(--bg); transition: border-color 0.15s, transform 0.15s var(--ease), box-shadow 0.15s; animation: ext-in 0.25s var(--ease); }
  .card:hover { border-color: var(--line-strong); transform: translateY(-1px); box-shadow: var(--shadow-sm); }
  .card-head { display: flex; align-items: center; gap: 6px; min-width: 0; }
  .card-head strong { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13.5px; }
  .card p { margin: 0; color: var(--muted); font-size: 12.5px; line-height: 1.5; display: -webkit-box; -webkit-line-clamp: 3; line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
  .meta { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; color: var(--subtle); font-size: 11.5px; }
  .card-foot { margin-top: auto; display: flex; align-items: center; gap: 8px; padding-top: 4px; }
  .card-foot code { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--subtle); font-size: 11px; }
  .more { display: flex; justify-content: center; }
</style>
