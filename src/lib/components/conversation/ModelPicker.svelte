<script lang="ts">
  import { tick } from 'svelte';
  import { Brain, Check, ChevronDown, Image, Search, Star } from '@lucide/svelte';
  import type { ModelInfo } from '../../types';
  import { formatCost, formatTokens, matchesModel, modelKey, providerHue, providerLabel } from './model-utils';

  interface Props {
    models: ModelInfo[];
    current?: ModelInfo | null;
    /** `provider/id` of the new-thread default, if one is set. */
    defaultKey?: string | null;
    onSelect: (key: string) => void | Promise<void>;
    onMakeDefault?: (model: ModelInfo) => void | Promise<void>;
    /** Open above the trigger (composer) or below it (settings). */
    placement?: 'up' | 'down';
    label?: string;
  }

  let { models, current = null, defaultKey = null, onSelect, onMakeDefault, placement = 'up', label = 'Select model' }: Props = $props();

  let open = $state(false);
  let query = $state('');
  let provider = $state('');
  let active = $state(0);
  let root = $state<HTMLDivElement>();
  let search = $state<HTMLInputElement>();
  let list = $state<HTMLDivElement>();
  let trigger = $state<HTMLButtonElement>();
  /** Fixed position: the composer clips overflow, so the panel escapes it. */
  let panelStyle = $state('');
  const listId = `model-list-${Math.random().toString(36).slice(2, 8)}`;

  const currentKey = $derived(current ? modelKey(current) : null);
  const currentContext = $derived(formatTokens(current?.contextWindow));
  const byName = (a: ModelInfo, b: ModelInfo) => (a.name || a.id).localeCompare(b.name || b.id);

  /** Providers, the current one first, each with its models. */
  const providers = $derived.by(() => {
    const grouped = new Map<string, ModelInfo[]>();
    for (const model of models) grouped.set(model.provider, [...(grouped.get(model.provider) ?? []), model]);
    return [...grouped.entries()]
      .map(([id, list]) => ({ id, label: providerLabel(id), hue: providerHue(id), models: list.sort(byName) }))
      .sort((a, b) => Number(b.id === current?.provider) - Number(a.id === current?.provider) || a.label.localeCompare(b.label));
  });
  const searching = $derived(query.trim().length > 0);
  /** Search spans every provider; otherwise the selected provider's models. */
  const sections = $derived.by(() => {
    if (!searching) {
      const selected = providers.find(entry => entry.id === provider) ?? providers[0];
      return selected ? [selected] : [];
    }
    return providers
      .map(entry => ({ ...entry, models: entry.models.filter(model => matchesModel(model, query)) }))
      .filter(entry => entry.models.length > 0);
  });
  const flat = $derived(sections.flatMap(section => section.models));
  const matchCounts = $derived(new Map(providers.map(entry => [entry.id, searching ? entry.models.filter(model => matchesModel(model, query)).length : entry.models.length])));

  function monogram(text: string): string {
    const words = text.split(/\s+/).filter(Boolean);
    return (words.length > 1 ? words[0][0] + words[1][0] : text.slice(0, 2)).toUpperCase();
  }

  function place() {
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    // Above the whole composer box, not just the trigger inside it.
    const anchor = placement === 'up' ? (trigger.closest('.box')?.getBoundingClientRect() ?? rect) : rect;
    const width = Math.min(600, innerWidth - 32);
    const left = Math.max(16, Math.min(anchor.left, innerWidth - width - 16));
    const room = placement === 'up' ? anchor.top - 22 : innerHeight - rect.bottom - 22;
    // Sized to the largest provider (or the rail), so switching doesn't jump.
    const rows = Math.max(1, ...providers.map(entry => entry.models.length));
    const needed = Math.max(44 + 36 + rows * 46 + 16, 44 + providers.length * 38 + 16);
    const height = Math.round(Math.min(room, Math.max(240, Math.min(460, needed))));
    panelStyle =
      placement === 'up'
        ? `left:${left}px;width:${width}px;bottom:${innerHeight - anchor.top + 8}px;height:${height}px`
        : `left:${left}px;width:${width}px;top:${rect.bottom + 8}px;height:${height}px`;
  }

  async function show() {
    place();
    open = true;
    query = '';
    provider = current?.provider ?? providers[0]?.id ?? '';
    await tick();
    active = Math.max(0, flat.findIndex(model => modelKey(model) === currentKey));
    search?.focus();
    scrollActive();
  }

  function hide(refocus = true) {
    open = false;
    if (refocus) trigger?.focus();
  }

  function choose(model: ModelInfo) {
    hide();
    if (modelKey(model) !== currentKey) void onSelect(modelKey(model));
  }

  function selectProvider(id: string) {
    provider = id;
    query = '';
    active = 0;
    void tick().then(() => {
      const index = flat.findIndex(model => modelKey(model) === currentKey);
      active = Math.max(0, index);
      list?.scrollTo({ top: 0 });
      scrollActive();
    });
  }

  function scrollActive() {
    void tick().then(() => list?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }));
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!flat.length) return;
      active = (active + (event.key === 'ArrowDown' ? 1 : -1) + flat.length) % flat.length;
      scrollActive();
    } else if ((event.key === 'ArrowLeft' || event.key === 'ArrowRight') && !searching && providers.length > 1) {
      event.preventDefault();
      const at = providers.findIndex(entry => entry.id === provider);
      selectProvider(providers[(at + (event.key === 'ArrowRight' ? 1 : -1) + providers.length) % providers.length].id);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      if (flat[active]) choose(flat[active]);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      if (searching) { query = ''; active = 0; }
      else hide();
    }
  }

  function onWindowPointer(event: PointerEvent) {
    if (open && root && !root.contains(event.target as Node)) hide(false);
  }
</script>

<svelte:window onpointerdown={onWindowPointer} onresize={() => open && hide(false)} />

<div class="picker" bind:this={root}>
  <button
    bind:this={trigger}
    type="button"
    class="trigger"
    aria-haspopup="dialog"
    aria-expanded={open}
    aria-label={label}
    title={current ? `${current.name} · ${providerLabel(current.provider)}${currentContext ? ` · ${currentContext} context` : ''}` : label}
    onclick={() => (open ? hide() : void show())}
  >
    {#if current}<span class="mono small" style={`--hue:${providerHue(current.provider)}`} aria-hidden="true">{monogram(providerLabel(current.provider))}</span>{/if}
    <span class="trigger-name">{current?.name ?? 'Default model'}</span>
    {#if currentContext}<span class="trigger-context">{currentContext}</span>{/if}
    <ChevronDown size={11} strokeWidth={2} />
  </button>

  {#if open}
    <div class="panel" class:up={placement === 'up'} role="dialog" aria-label="Choose a model" style={panelStyle}>
      <div class="search">
        <Search size={14} strokeWidth={2} />
        <input
          bind:this={search}
          bind:value={query}
          oninput={() => { active = 0; scrollActive(); }}
          onkeydown={onKeydown}
          placeholder="Search all models…"
          aria-label="Search models"
          role="combobox"
          aria-expanded="true"
          aria-controls={listId}
          aria-activedescendant={flat[active] ? `${listId}-${active}` : undefined}
        />
        <span class="keys" aria-hidden="true"><kbd>↑↓</kbd> models{#if providers.length > 1} · <kbd>←→</kbd> providers{/if}</span>
      </div>
      <div class="body">
        {#if providers.length > 1}
          <div class="rail" role="tablist" aria-label="Providers" aria-orientation="vertical">
            {#each providers as entry (entry.id)}
              {@const count = matchCounts.get(entry.id) ?? 0}
              <button
                type="button"
                role="tab"
                class="provider"
                class:on={!searching && entry.id === (sections[0]?.id ?? '')}
                class:dim={searching && count === 0}
                aria-selected={!searching && entry.id === (sections[0]?.id ?? '')}
                onclick={() => selectProvider(entry.id)}
              >
                <span class="mono" style={`--hue:${entry.hue}`} aria-hidden="true">{monogram(entry.label)}</span>
                <span class="provider-name">{entry.label}</span>
                {#if entry.id === current?.provider}<span class="live" title="Current model's provider"></span>{/if}
                <span class="provider-count">{count}</span>
              </button>
            {/each}
          </div>
        {/if}
        <div class="list" id={listId} role="listbox" aria-label="Models" bind:this={list}>
          {#each sections as section (section.id)}
            <div class="group" role="group" aria-label={section.label}>
              <div class="group-head">
                <span class="mono small" style={`--hue:${section.hue}`} aria-hidden="true">{monogram(section.label)}</span>
                <span>{section.label}</span>
                <span class="group-count">{section.models.length} model{section.models.length === 1 ? '' : 's'}</span>
              </div>
              {#each section.models as model (modelKey(model))}
                {@const index = flat.indexOf(model)}
                {@const key = modelKey(model)}
                {@const context = formatTokens(model.contextWindow)}
                {@const cost = formatCost(model.cost)}
                <div
                  id={`${listId}-${index}`}
                  class="row"
                  class:active={index === active}
                  class:current={key === currentKey}
                  data-active={index === active}
                  role="option"
                  tabindex="-1"
                  aria-selected={key === currentKey}
                  onpointermove={() => (active = index)}
                  onclick={() => choose(model)}
                  onkeydown={onKeydown}
                >
                  <span class="main">
                    <span class="name">
                      {model.name || model.id}
                      {#if key === currentKey}<span class="check" aria-label="Current"><Check size={12} strokeWidth={2.6} /></span>{/if}
                      {#if key === defaultKey}<span class="badge">Default</span>{/if}
                    </span>
                    <span class="meta">
                      {#if cost}<span class:free={cost === 'Free'} title="USD per million input / output tokens">{cost}</span>{/if}
                      <span class="id">{model.id}</span>
                    </span>
                  </span>
                  <span class="traits">
                    {#if model.reasoning}<span class="trait" title="Reasoning"><Brain size={12} strokeWidth={2} /></span>{/if}
                    {#if model.images}<span class="trait" title="Accepts images"><Image size={12} strokeWidth={2} /></span>{/if}
                    {#if context}<span class="context" title={`${model.contextWindow?.toLocaleString()} token context window${model.maxTokens ? `, up to ${model.maxTokens.toLocaleString()} output tokens` : ''}`}>{context}</span>{/if}
                  </span>
                  {#if onMakeDefault}
                    <button
                      type="button"
                      class="make-default"
                      class:on={key === defaultKey}
                      aria-label={key === defaultKey ? `${model.name} is the default for new threads` : `Make ${model.name} the default for new threads`}
                      title={key === defaultKey ? 'Default for new threads' : 'Make default for new threads'}
                      disabled={key === defaultKey}
                      onclick={(event) => { event.stopPropagation(); void onMakeDefault(model); }}
                    >
                      <Star size={13} strokeWidth={2} fill={key === defaultKey ? 'currentColor' : 'none'} />
                    </button>
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
          {#if !flat.length}<p class="empty">{searching ? `No models match “${query}”.` : 'No models.'}</p>{/if}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    min-width: 0;
  }
  .mono {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    background: hsl(var(--hue) 70% 55% / 0.16);
    /* Leans toward the theme's text color, so it reads in light and dark. */
    color: color-mix(in srgb, hsl(var(--hue) 70% 50%) 70%, var(--text));
    font-size: 9.5px;
    font-weight: 700;
    letter-spacing: 0.02em;
  }
  .mono.small {
    width: 16px;
    height: 16px;
    border-radius: 4px;
    font-size: 7.5px;
  }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    height: 26px;
    padding: 0 8px 0 5px;
    border: 0;
    border-radius: 999px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
  }
  .trigger:hover,
  .trigger[aria-expanded='true'] {
    background: var(--surface-2);
    color: var(--text);
  }
  .trigger :global(svg) {
    flex: none;
  }
  .trigger-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .trigger-context {
    flex: none;
    padding: 0 5px;
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--subtle);
    font-size: 10.5px;
    font-variant-numeric: tabular-nums;
  }
  .trigger:hover .trigger-context,
  .trigger[aria-expanded='true'] .trigger-context {
    background: var(--surface-3);
  }

  .panel {
    position: fixed;
    z-index: 40;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid var(--line-strong);
    border-radius: 14px;
    background: var(--elevated);
    box-shadow: var(--shadow);
    animation: picker-in 0.16s var(--ease);
    transform-origin: bottom left;
  }
  .panel:not(.up) {
    transform-origin: top left;
  }
  @keyframes picker-in {
    from {
      opacity: 0;
      transform: translateY(4px) scale(0.985);
    }
  }
  .search {
    display: flex;
    align-items: center;
    gap: 9px;
    flex: none;
    height: 44px;
    padding: 0 14px;
    border-bottom: 1px solid var(--line);
    color: var(--subtle);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--text);
    font-size: 13.5px;
    outline: none;
  }
  .search input:focus,
  .search input:focus-visible {
    outline: none;
    box-shadow: none;
  }
  .keys {
    flex: none;
    color: var(--subtle);
    font-size: 11px;
  }
  .keys kbd {
    font-size: 10px;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .rail {
    flex: none;
    width: 190px;
    overflow-y: auto;
    padding: 6px;
    border-right: 1px solid var(--line);
    background: color-mix(in srgb, var(--surface) 60%, transparent);
  }
  .provider {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 36px;
    padding: 0 8px;
    border: 0;
    border-radius: 8px;
    background: none;
    color: var(--muted);
    font-size: 12.5px;
    font-weight: 500;
    text-align: left;
    transition: background 0.12s, color 0.12s, opacity 0.12s;
  }
  .provider:hover {
    background: var(--surface-2);
    color: var(--text);
  }
  .provider.on {
    background: var(--elevated);
    color: var(--text);
    box-shadow: var(--shadow-sm), 0 0 0 1px var(--line);
  }
  .provider.dim {
    opacity: 0.4;
  }
  .provider-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .live {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }
  .provider-count {
    flex: none;
    min-width: 20px;
    padding: 0 5px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--subtle);
    font-size: 10.5px;
    line-height: 17px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }

  .list {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 4px 6px 8px;
  }
  .group + .group {
    margin-top: 6px;
  }
  .group-head {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 8px 8px 6px;
    background: var(--elevated);
    color: var(--text);
    font-size: 12px;
    font-weight: 600;
  }
  .group-count {
    margin-left: auto;
    color: var(--subtle);
    font-size: 11px;
    font-weight: 500;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 44px;
    padding: 6px 8px 6px 10px;
    border-radius: 9px;
    cursor: pointer;
    transition: background 0.1s;
  }
  .row.active {
    background: var(--surface-2);
  }
  .row.current {
    background: var(--accent-bg);
  }
  .row.current.active {
    background: color-mix(in srgb, var(--accent-bg) 80%, var(--surface-2));
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    color: var(--text);
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
  }
  .check {
    display: inline-flex;
    color: var(--accent);
  }
  .badge {
    padding: 0 6px;
    border-radius: 999px;
    background: var(--warn-bg);
    color: var(--warn);
    font-size: 10px;
    font-weight: 700;
    line-height: 16px;
  }
  .meta {
    display: flex;
    gap: 8px;
    min-width: 0;
    color: var(--subtle);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .meta .free {
    color: var(--good);
    font-weight: 600;
  }
  .id {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    font-family: var(--mono);
    font-size: 10.5px;
    opacity: 0.8;
  }
  .traits {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .trait {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 20px;
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--muted);
  }
  .context {
    min-width: 38px;
    padding: 0 6px;
    border-radius: 6px;
    background: var(--surface-2);
    color: var(--text);
    font-size: 11px;
    font-weight: 600;
    line-height: 20px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .row.current .trait,
  .row.current .context {
    background: color-mix(in srgb, var(--elevated) 70%, transparent);
  }
  .make-default {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: 7px;
    background: none;
    color: var(--subtle);
    opacity: 0;
    transition: opacity 0.1s, color 0.1s, background 0.1s;
  }
  .row:hover .make-default,
  .row.active .make-default,
  .make-default.on,
  .make-default:focus-visible {
    opacity: 1;
  }
  .make-default:hover:not(:disabled) {
    background: var(--surface-3);
    color: var(--warn);
  }
  .make-default.on {
    color: var(--warn);
  }
  .empty {
    margin: 24px 12px;
    color: var(--muted);
    font-size: 12.5px;
    text-align: center;
  }
  @media (max-width: 560px) {
    .rail {
      width: 60px;
    }
    .provider-name,
    .provider-count,
    .keys {
      display: none;
    }
  }
</style>
