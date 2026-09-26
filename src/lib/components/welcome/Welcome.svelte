<script lang="ts">
  import { ArrowUpRight, Bot, FolderPlus, MessageSquare } from '@lucide/svelte';
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
      <div class="identity"><img src="/app-icon.svg" alt="" width="40" height="40" /><span>Your local workspace</span></div>
      <h1>Welcome to πDesk</h1>
      <p>A little more room to think, build, and review.<br />Your Pi agents, right next to your code.</p>
      <section class="start" aria-label="Start">
        <button class="add-project" disabled={busy} onclick={onAddProject}><FolderPlus size={17} strokeWidth={1.8}/> Add project…</button>
        <button class="intern" onclick={onOpenIntern}><Bot size={16} strokeWidth={1.8}/> Open Pi Intern <ArrowUpRight size={14} /></button>
      </section>
    </header>
    <div class="columns">
      <section class="recent" aria-label="Last used">
        <div class="section-heading"><h2>Pick up where you left off</h2><span>Recent threads</span></div>
        {#each lastUsed as thread (thread.id)}
          <button class="row" onclick={() => onOpenThread(thread)} title={thread.title || 'New thread'}>
            <span class="thread-glyph" aria-hidden="true"><MessageSquare size={16} strokeWidth={1.8}/></span>
            <span class="name">{thread.title || 'New thread'}</span>
            <span class="meta">{projectName.get(thread.projectId)} · {ago(thread.lastViewedAt)}</span>
            <ArrowUpRight size={15} class="row-arrow" aria-hidden="true" />
          </button>
        {:else}
          <div class="empty-recent"><MessageSquare size={21} strokeWidth={1.5} aria-hidden="true"/><p>Your next idea starts here.</p><span>Add a project and start a thread. You can return to it here anytime.</span></div>
        {/each}
      </section>
      <section class="projects" aria-label="Your projects">
        <div class="section-heading"><h2>Your projects</h2><span>Local folders</span></div>
        {#each projects.slice(0, MAX_PROJECTS) as project (project.id)}
          <button class="row" onclick={() => onOpenProject(project)} title={project.path}>
            <span class="glyph" aria-hidden="true">{project.displayName[0]?.toUpperCase()}</span>
            <span class="name">{project.displayName}</span>
            <span class="meta">{project.path}</span>
          </button>
        {:else}
          <p class="none">Add a local folder to give your agent a place to work.</p>
        {/each}
        {#if projects.length > MAX_PROJECTS}<p class="none">{projects.length - MAX_PROJECTS} more in the sidebar · <kbd>⌘</kbd><kbd>K</kbd> to search</p>{/if}
      </section>
    </div>
  </div>
</div>

<style>
  .welcome { flex:1; overflow:auto; animation:ui-rise .25s var(--ease); }
  .inner { max-width:920px; margin:0 auto; padding:48px 40px 40px; }
  header { padding-bottom:32px; border-bottom:1px solid var(--line); }
  .identity { display:flex; align-items:center; gap:12px; margin-bottom:22px; color:var(--muted); font-size:12px; font-weight:500; letter-spacing:.025em; }
  .identity img { border-radius:11px; box-shadow:var(--shadow-sm); }
  h1 { font-size:32px; font-weight:650; letter-spacing:-.035em; margin:0; line-height:1.2; }
  header p { color:var(--muted); font-size:14px; line-height:1.7; margin:12px 0 0; }
  .start { display:flex; align-items:center; flex-wrap:wrap; gap:12px; margin-top:24px; }
  .start button { display:inline-flex; align-items:center; justify-content:center; gap:8px; min-height:38px; padding:0 14px; border:1px solid transparent; border-radius:var(--radius); font-size:13px; font-weight:600; }
  .add-project { background:var(--accent-strong); color:var(--on-accent); box-shadow:var(--shadow-sm); }
  .add-project:hover:not(:disabled) { filter:brightness(1.08); }
  .start .intern { border-color:var(--line-strong); background:var(--bg); color:var(--muted); font-weight:500; }
  .intern:hover { background:var(--surface); color:var(--text); }
  .columns { display:grid; grid-template-columns:minmax(0,1.2fr) minmax(0,1fr); gap:32px; margin-top:28px; }
  .section-heading { display:flex; flex-direction:column; gap:4px; margin-bottom:14px; }
  h2 { font-size:14px; font-weight:600; letter-spacing:-.01em; color:var(--text); margin:0; }
  .section-heading > span { font-size:12px; color:var(--subtle); }
  .row { display:grid; grid-template-columns:auto minmax(0,1fr) auto; grid-template-areas:'icon name arrow' 'icon meta arrow'; column-gap:12px; row-gap:3px; align-items:center; width:100%; padding:12px; border:1px solid transparent; border-radius:var(--radius); background:none; color:var(--text); text-align:left; font:13px var(--font); }
  .row:hover { background:var(--surface); border-color:var(--line); }
  .row:focus-visible { outline:none; box-shadow:var(--focus-ring); }
  .recent .row { background:var(--surface); border-color:var(--line); margin-bottom:8px; }
  .recent .row:hover { background:var(--surface-2); border-color:var(--line-strong); }
  .glyph, .thread-glyph { grid-area:icon; width:32px; height:32px; display:flex; align-items:center; justify-content:center; border-radius:9px; background:var(--accent-bg); color:var(--accent); font-size:12px; font-weight:600; }
  .thread-glyph { background:var(--elevated); color:var(--muted); box-shadow:var(--shadow-sm); }
  .row :global(.row-arrow) { grid-area:arrow; color:var(--subtle); }
  .row:hover :global(.row-arrow) { color:var(--accent); }
  .name { grid-area:name; font-weight:500; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .meta { grid-area:meta; color:var(--subtle); font-size:12px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .none { color:var(--muted); font-size:13px; margin:4px 0; line-height:1.6; }
  .empty-recent { padding:24px; border:1px dashed var(--line-strong); border-radius:var(--radius-lg); color:var(--muted); }
  .empty-recent p { color:var(--text); font-size:14px; font-weight:500; margin:12px 0 6px; }
  .empty-recent span { font-size:13px; line-height:1.6; }
  @container main (max-width:700px) {
    .inner { padding:32px 24px; }
    .columns { grid-template-columns:minmax(0,1fr); gap:24px; }
    h1 { font-size:28px; }
  }
</style>
