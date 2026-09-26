<script lang="ts">
  import CommandCard from './CommandCard.svelte';
  import EditCard from './EditCard.svelte';
  import GenericCard from './GenericCard.svelte';
  import PlanCard from './PlanCard.svelte';
  import McpCard from './McpCard.svelte';
  import { loadMcpServers, mcpCall } from './mcp-tools.svelte';
  import ReadCard from './ReadCard.svelte';
  import SearchCard from './SearchCard.svelte';
  import TodoCard from './TodoCard.svelte';
  import WebCard from './WebCard.svelte';
  import type { ToolItem } from './tool-utils';

  interface Props {
    item: ToolItem;
    onShowChanges?: (path?: string) => void;
  }

  let { item, onShowChanges }: Props = $props();

  // MCP calls (the adapter's tools, or `<server>_<tool>` direct tools) get their own card.
  void loadMcpServers();
  const mcp = $derived(mcpCall(item));

  type Category = 'command' | 'read' | 'search' | 'edit' | 'write' | 'web' | 'todo' | 'plan-approval' | 'generic';

  const CATEGORY_BY_NAME: Record<string, Category> = {
    bash: 'command',
    sh: 'command',
    shell: 'command',
    exec: 'command',
    execute: 'command',
    executebash: 'command',
    executecommand: 'command',
    execute_command: 'command',
    run: 'command',
    runcommand: 'command',
    run_command: 'command',
    runterminalcommand: 'command',
    terminal: 'command',
    process: 'command',
    read: 'read',
    readfile: 'read',
    read_file: 'read',
    view: 'read',
    open: 'read',
    cat: 'read',
    grep: 'search',
    search: 'search',
    find: 'search',
    glob: 'search',
    rg: 'search',
    ripgrep: 'search',
    ls: 'search',
    list: 'search',
    listfiles: 'search',
    list_files: 'search',
    searchfiles: 'search',
    search_files: 'search',
    codesearch: 'search',
    edit: 'edit',
    editfile: 'edit',
    edit_file: 'edit',
    applypatch: 'edit',
    apply_patch: 'edit',
    patch: 'edit',
    replace: 'edit',
    strreplace: 'edit',
    str_replace: 'edit',
    write: 'write',
    writefile: 'write',
    write_file: 'write',
    createfile: 'write',
    create_file: 'write',
    websearch: 'web',
    web_search: 'web',
    searchweb: 'web',
    fetch: 'web',
    fetchurl: 'web',
    fetch_url: 'web',
    fetchcontent: 'web',
    fetch_content: 'web',
    browse: 'web',
    browser: 'web',
    openurl: 'web',
    open_url: 'web',
    readurl: 'web',
    read_url: 'web',
    todo: 'todo',
    todowrite: 'todo',
    todo_write: 'todo',
    todolist: 'todo',
    todo_list: 'todo',
    plan: 'todo',
    updateplan: 'todo',
    update_plan: 'todo',
    request_auto: 'plan-approval',
  };

  function normalize(name: string): string {
    return name.toLowerCase().replace(/[^a-z_]/g, '');
  }

  const category = $derived.by((): Category => {
    const name = normalize(item.toolName);
    const mapped = CATEGORY_BY_NAME[name];
    return mapped ?? 'generic';
  });
</script>

{#if mcp}
  <McpCard {item} call={mcp} />
{:else if category === 'command'}
  <CommandCard {item} />
{:else if category === 'read'}
  <ReadCard {item} />
{:else if category === 'search'}
  <SearchCard {item} />
{:else if category === 'edit'}
  <EditCard {item} {onShowChanges} />
{:else if category === 'write'}
  <EditCard {item} write {onShowChanges} />
{:else if category === 'web'}
  <WebCard {item} />
{:else if category === 'todo'}
  <TodoCard {item} />
{:else if category === 'plan-approval'}
  <PlanCard {item} />
{:else}
  <GenericCard {item} />
{/if}
