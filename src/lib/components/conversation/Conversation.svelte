<script lang="ts">
  import type { SessionView, UiResponse } from '../../types';
  import Composer from './Composer.svelte';
  import RequestCard from './RequestCard.svelte';
  import Transcript from './Transcript.svelte';

  interface Props {
    view: SessionView;
    onSend: (message: string, mode: 'prompt' | 'steer' | 'follow_up') => void | Promise<void>;
    onAbort: () => void | Promise<void>;
    onShowChanges: (path?: string) => void;
    onShowAgents: () => void;
    onRespond: (requestId: string, response: UiResponse) => void | Promise<void>;
  }

  let { view, onSend, onAbort, onShowChanges, onShowAgents, onRespond }: Props = $props();
</script>

<section class="conversation" aria-label="Conversation">
  <Transcript items={view.items} agents={view.agents} {onShowChanges} {onShowAgents} />

  {#if view.pendingRequests.length > 0}
    <div class="requests" aria-live="polite">
      {#each view.pendingRequests as request (request.id)}
        <RequestCard {request} {onRespond} />
      {/each}
    </div>
  {/if}


  <Composer
    threadId={view.threadId}
    status={view.status}
    commands={view.commands}
    {onSend}
    {onAbort}
  />
</section>

<style>
  .conversation {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
  }
  .requests {
    padding: 0 16px;
    max-height: 45%;
    overflow-y: auto;
  }
</style>
