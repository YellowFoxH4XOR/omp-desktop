<script lang="ts">
  import { tick } from 'svelte';
  import { Brain, Check, ChevronDown, Cpu, Image, Search, Star } from '@lucide/svelte';
  import type { ModelInfo } from '../../types';
  import { formatCost, formatTokens, matchesModel, modelKey, providerLabel } from './model-utils';

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

  const groups = $derived.by(() => {
    const byProvider = new Map<string, ModelInfo[]>();
    for (const model of models) {
      if (!matchesModel(model, query)) continue;
      const list = byProvider.get(model.provider) ?? [];
      list.push(model);
      byProvider.set(model.provider, list);
    }
    return [...byProvider.entries()]
      .map(([provider, list]) => ({
        provider,
        label: providerLabel(provider),
        models: list.sort((a, b) => (a.name || a.id).localeCompare(b.name || b.id)),
      }))
      .sort((a, b) =>
        Number(b.provider === current?.provider) - Number(a.provider === current?.provider) || a.label.localeCompare(b.label),
      );
  });
  const flat = $derived(groups.flatMap((group) => group.models));

  function place() {
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const width = Math.min(400, innerWidth - 32);
    const left = Math.max(16, Math.min(rect.left, innerWidth - width - 16));
    panelStyle =
      placement === 'up'
        ? `left:${left}px;width:${width}px;bottom:${innerHeight - rect.top + 6}px;max-height:${Math.min(440, rect.top - 22)}px`
        : `left:${left}px;width:${width}px;top:${rect.bottom + 6}px;max-height:${Math.min(440, innerHeight - rect.bottom - 22)}px`;
  }

  async function show() {
    place();
    open = true;
    query = '';
    await tick();
    active = Math.max(0, flat.findIndex((model) => modelKey(model) === currentKey));
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

  function scrollActive() {
    void tick().then(() => list?.querySelector('[data-active="true"]')?.scrollIntoView({ block: 'nearest' }));
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!flat.length) return;
      active = (active + (event.key === 'ArrowDown' ? 1 : -1) + flat.length) % flat.length;
      scrollActive();
    } else if (event.key === 'Enter') {
      event.preventDefault();
      if (flat[active]) choose(flat[active]);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      hide();
    }
  }

  function onWindowPointer(event: PointerEvent) {
    if (open && root && !root.contains(event.target as Node)) hide(false);
  }
</script>

<svelte:window onpointerdown={onWindowPointer} onresize={() => open && hide(false)} />

<div class="picker" class:up={placement === 'up'} bind:this={root}>
  <button
    bind:this={trigger}
    type="button"
    class="trigger"
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label={label}
    title={current ? `${current.name} · ${providerLabel(current.provider)}${currentContext ? ` · ${currentContext} context` : ''}` : label}
    onclick={() => (open ? hide() : void show())}
  >
    <Cpu size={12} strokeWidth={2} />
    <span class="trigger-name">{current?.name ?? 'Default model'}</span>
    {#if currentContext}<span class="trigger-context">{currentContext}</span>{/if}
    <ChevronDown size={11} strokeWidth={2} />
  </button>

  {#if open}
    <div class="panel" role="dialog" aria-label="Choose a model" style={panelStyle}>
      <div class="search">
        <Search size={13} strokeWidth={2} />
        <input
          bind:this={search}
          bind:value={query}
          oninput={() => { active = 0; scrollActive(); }}
          onkeydown={onKeydown}
          placeholder="Search models or providers…"
          aria-label="Search models"
          role="combobox"
          aria-expanded="true"
          aria-controls={listId}
          aria-activedescendant={flat[active] ? `${listId}-${active}` : undefined}
        />
        <span class="count">{flat.length}</span>
      </div>
      <div class="list" id={listId} role="listbox" aria-label="Models" bind:this={list}>
        {#each groups as group (group.provider)}
          <div class="group" role="group" aria-label={group.label}>
            <div class="group-head"><span>{group.label}</span><span class="group-count">{group.models.length}</span></div>
            {#each group.models as model (modelKey(model))}
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
                <span class="check" aria-hidden="true">{#if key === currentKey}<Check size={13} strokeWidth={2.4} />{/if}</span>
                <span class="main">
                  <span class="name">
                    {model.name || model.id}
                    {#if key === defaultKey}<span class="badge default">Default</span>{/if}
                  </span>
                  <span class="meta">
                    <span class="id">{model.id}</span>
                    {#if cost}<span class="cost" title="USD per million input / output tokens">{cost}</span>{/if}
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
                    <Star size={12} strokeWidth={2} fill={key === defaultKey ? 'currentColor' : 'none'} />
                  </button>
                {/if}
              </div>
            {/each}
          </div>
        {/each}
        {#if !flat.length}<p class="empty">No models match “{query}”.</p>{/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    min-width: 0;
  }
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 100%;
    height: 26px;
    padding: 0 8px;
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
    border-radius: var(--radius-lg);
    background: var(--elevated);
    box-shadow: var(--shadow);
    overflow: hidden;
    animation: ui-pop 0.12s var(--ease);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 40px;
    padding: 0 12px;
    border-bottom: 1px solid var(--line);
    color: var(--subtle);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--text);
    font-size: 13px;
  }
  .search input:focus-visible {
    box-shadow: none;
  }
  .count {
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px;
  }
  .group + .group {
    margin-top: 4px;
    padding-top: 4px;
    border-top: 1px solid var(--line);
  }
  .group-head {
    position: sticky;
    top: -4px;
    z-index: 1;
    display: flex;
    justify-content: space-between;
    padding: 6px 8px 4px;
    background: var(--elevated);
    color: var(--subtle);
    font-size: 11px;
    font-weight: 600;
  }
  .group-count {
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 6px 6px 4px;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .row.active {
    background: var(--accent-bg);
  }
  .check {
    flex: none;
    width: 16px;
    display: inline-flex;
    justify-content: center;
    color: var(--accent);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-size: 12.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.current .name {
    font-weight: 600;
  }
  .badge {
    flex: none;
    padding: 0 5px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 600;
  }
  .badge.default {
    color: var(--warn);
    background: var(--warn-bg);
  }
  .meta {
    display: flex;
    gap: 8px;
    min-width: 0;
    color: var(--subtle);
    font-size: 11px;
  }
  .id {
    font-family: var(--mono);
    font-size: 10.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cost {
    flex: none;
    font-variant-numeric: tabular-nums;
  }
  .traits {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--subtle);
  }
  .trait {
    display: inline-flex;
  }
  .context {
    min-width: 40px;
    padding: 1px 6px;
    border-radius: 5px;
    background: var(--surface-2);
    color: var(--muted);
    font-size: 11px;
    font-weight: 500;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
  .make-default {
    flex: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--subtle);
    opacity: 0;
  }
  .row:hover .make-default,
  .row.active .make-default,
  .make-default.on,
  .make-default:focus-visible {
    opacity: 1;
  }
  .make-default:hover:not(:disabled) {
    color: var(--warn);
    background: var(--surface-2);
  }
  .make-default.on {
    color: var(--warn);
  }
  .make-default:disabled {
    cursor: default;
  }
  .empty {
    margin: 0;
    padding: 18px 12px;
    color: var(--muted);
    font-size: 12px;
    text-align: center;
  }
</style>
