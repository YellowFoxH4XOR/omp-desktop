<script lang="ts">
  import type { SessionView, UiResponse } from '../../types';
  import Composer from './Composer.svelte';
  import RequestCard from './RequestCard.svelte';
  import Transcript from './Transcript.svelte';
  import { planOf, type PlanActions } from '../../plan';

  interface Props {
    view: SessionView;
    onSend: (message: string, mode: 'prompt' | 'steer' | 'follow_up') => void | Promise<void>;
    onAbort: () => void | Promise<void>;
    onShowChanges: (path?: string) => void;
    onRespond: (requestId: string, response: UiResponse) => void | Promise<void>;
    onSetModel?: (value: string) => void | Promise<void>;
    onSetEffort?: (level: string) => void | Promise<void>;
    defaultModelKey?: string | null;
    onMakeDefault?: (model: import('../../types').ModelInfo) => void | Promise<void>;
    agentMode?: import('../../types').ThreadMode;
    onSetMode?: (mode: import('../../types').ThreadMode) => void | Promise<void>;
    /** A pending plan approval, answered from the composer strip. */
    plan?: PlanActions;
  }

  let { view, onSend, onAbort, onShowChanges, onRespond, onSetModel, onSetEffort, defaultModelKey = null, onMakeDefault, agentMode, onSetMode, plan }: Props = $props();
  const otherRequests = $derived(plan ? view.pendingRequests.filter(request => planOf(request) === null) : view.pendingRequests);
</script>

<section class="conversation" aria-label="Conversation">
  <Transcript items={view.items} status={view.status} {onShowChanges} />

  <div class="dock">
    {#if otherRequests.length > 0}
      <div class="requests" aria-live="polite">
        {#each otherRequests as request, requestIndex (requestIndex + ':' + request.id)}
          <RequestCard {request} {onRespond} />
        {/each}
      </div>
    {/if}

    <Composer
      threadId={view.threadId}
      status={view.status}
      commands={view.commands}
      model={view.model}
      models={view.capabilities.modelSwitching ? view.models : []}
      effort={view.effort}
      levels={view.capabilities.effortLevels ? view.levels : []}
      contextUsage={view.contextUsage}
      {onSend}
      {onAbort}
      {onSetModel}
      {onSetEffort}
      {defaultModelKey}
      {onMakeDefault}
      {agentMode}
      {onSetMode}
      approval={plan}
    />
  </div>
</section>

<style>
  .conversation {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    background: var(--bg);
  }
  .dock {
    flex: none;
    width: 100%;
    max-width: calc(var(--content-width) + 48px);
    margin: 0 auto;
    padding: 4px 24px 16px;
  }
  .requests {
    max-height: 48vh;
    overflow-y: auto;
    margin: 0 -24px;
    padding: 16px 24px 6px;
  }
</style>
