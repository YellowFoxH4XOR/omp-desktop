<script lang="ts">
  import { Bot, FolderPlus, MessageSquare } from '@lucide/svelte';
  import type { Project, Thread } from '$lib/types';
  import { ago } from '$lib/time';

  let { projects, recent, busy, onOpenProject, onOpenThread, onAddProject, onOpenIntern }: {
    projects: Project[];
    recent: Thread[];
    busy: boolean;
    onOpenProject: (project: Project) => void;
    onOpenThread: (thread: Thread) => void;
    onAddProject: () => void;
    onOpenIntern: () => void;
  } = $props();

  const MAX_PROJECTS = 12;
  const projectName = $derived(new Map(projects.map(project => [project.id, project.displayName])));
  const lastUsed = $derived(recent.filter(thread => projectName.has(thread.projectId)));

</script>

<div class="welcome">
  <div class="inner">
    <header>
      <h1>Welcome to πDesk</h1>
      <p>Local Pi agents next to your code, with Git-backed review.</p>
    </header>
    <div class="columns">
      <div>
        <section aria-label="Start">
          <h2>Start</h2>
          <button class="link" disabled={busy} onclick={onAddProject}><FolderPlus size={15} strokeWidth={1.8}/> Add project…</button>
          <button class="link" onclick={onOpenIntern}><Bot size={15} strokeWidth={1.8}/> Open Pi Intern</button>
        </section>
        <section aria-label="Your projects">
          <h2>Your projects</h2>
          {#each projects.slice(0, MAX_PROJECTS) as project (project.id)}
            <button class="row" onclick={() => onOpenProject(project)} title={project.path}>
              <span class="glyph" aria-hidden="true">{project.displayName[0]?.toUpperCase()}</span>
              <span class="name">{project.displayName}</span>
              <span class="meta">{project.path}</span>
            </button>
          {:else}
            <p class="none">No projects yet. Add a local folder; nothing is uploaded or copied.</p>
          {/each}
          {#if projects.length > MAX_PROJECTS}<p class="none">{projects.length - MAX_PROJECTS} more in the sidebar · <kbd>⌘</kbd><kbd>K</kbd> to search</p>{/if}
        </section>
      </div>
      <section aria-label="Last used">
        <h2>Last used</h2>
        {#each lastUsed as thread (thread.id)}
          <button class="row" onclick={() => onOpenThread(thread)} title={thread.title || 'New thread'}>
            <MessageSquare size={14} strokeWidth={1.8} class="thread-icon"/>
            <span class="name">{thread.title || 'New thread'}</span>
            <span class="meta">{projectName.get(thread.projectId)} · {ago(thread.lastViewedAt)}</span>
          </button>
        {:else}
          <p class="none">Threads you open will appear here.</p>
        {/each}
      </section>
    </div>
  </div>
</div>

<style>
  .welcome { flex:1; overflow:auto; animation:ui-rise .25s var(--ease); }
  .inner { max-width:880px; margin:0 auto; padding:64px 40px 40px; }
  h1 { font-size:28px; font-weight:600; letter-spacing:-.025em; margin:0; }
  header p { color:var(--muted); font-size:14px; margin:8px 0 0; }
  .columns { display:grid; grid-template-columns:minmax(0,1fr) minmax(0,1fr); gap:40px; margin-top:40px; }
  section + section { margin-top:28px; }
  h2 { font-size:12px; font-weight:600; letter-spacing:.02em; color:var(--subtle); text-transform:uppercase; margin:0 0 8px; }
  .link { display:flex; align-items:center; gap:8px; padding:5px 0; border:0; background:none; color:var(--accent); font:13px var(--font); cursor:pointer; }
  .link:hover:not(:disabled) { text-decoration:underline; }
  .link:disabled { opacity:.5; cursor:default; }
  .row { display:grid; grid-template-columns:auto minmax(0,1fr); grid-template-areas:'icon name' 'icon meta'; column-gap:10px; align-items:center; width:100%; padding:7px 8px; margin:0 -8px; border:0; border-radius:var(--radius-sm); background:none; color:var(--text); text-align:left; font:13px var(--font); cursor:pointer; box-sizing:content-box; }
  .row:hover { background:var(--surface); }
  .row:focus-visible { outline:none; box-shadow:var(--focus-ring); }
  .glyph, .row :global(.thread-icon) { grid-area:icon; }
  .glyph { width:22px; height:22px; display:flex; align-items:center; justify-content:center; border-radius:6px; background:var(--accent-bg); color:var(--accent); font-size:11px; font-weight:600; }
  .row :global(.thread-icon) { color:var(--subtle); margin:0 4px; }
  .name { grid-area:name; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .meta { grid-area:meta; color:var(--subtle); font-size:11.5px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .none { color:var(--subtle); font-size:12.5px; margin:4px 0; line-height:1.5; }
  @media (max-width:760px) {
    .inner { padding:40px 16px 24px; }
    .columns { grid-template-columns:minmax(0,1fr); gap:28px; }
  }
</style>
