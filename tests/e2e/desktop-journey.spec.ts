import { expect, test, type Page } from '@playwright/test';

const project = {
  id: 'project-1',
  path: '/tmp/pidesk-e2e',
  displayName: 'desktop-e2e',
  preferredHarness: 'pi',
  isGit: true,
  createdAt: '2026-01-01T00:00:00Z',
  lastOpenedAt: '2026-01-01T00:00:00Z',
};

function thread(id: string, title: string, viewedAt: string) {
  return {
    id,
    projectId: project.id,
    harness: 'pi',
    sessionId: `session-${id}`,
    sessionFile: `/tmp/${id}.jsonl`,
    cwd: project.path,
    title,
    pinned: false,
    archived: false,
    status: 'idle',
    createdAt: '2026-01-01T00:00:00Z',
    lastViewedAt: viewedAt,
    mode: 'auto',
  };
}

interface Scenario {
  piInstalled?: boolean;
  systemPiOnly?: boolean;
  manualInstall?: boolean;
  missingInstallPlan?: boolean;
  deferEvents?: boolean;
  titles?: [string, string];
  openDelay?: Record<string, number>;
  sendDelay?: number;
  rememberedThreadId?: string;
  /** Raw assistant text, used to exercise the markdown sanitizer. */
  assistantText?: string;
  /** Serve a two-provider model catalogue with model switching enabled. */
  models?: boolean;
  /** Raw get_messages history for thread `a`. */
  history?: unknown[];
}

const MODEL_CATALOGUE = [
  { provider: 'opencode-go', id: 'kimi-k2.6', name: 'Kimi K2.6', contextWindow: 262144, reasoning: true, images: true, cost: { input: 0.95, output: 4 } },
  { provider: 'opencode-go', id: 'glm-5.1', name: 'GLM-5.1', contextWindow: 200000, reasoning: true, cost: { input: 1, output: 3.2 } },
  { provider: 'opencode-go', id: 'deepseek-v4.1-flash', name: 'DeepSeek V4.1 Flash', contextWindow: 1048576, cost: { input: 0, output: 0 } },
  { provider: 'openai-codex', id: 'gpt-5.3-codex-spark', name: 'GPT-5.3 Codex Spark', contextWindow: 128000, reasoning: true, cost: { input: 1.75, output: 14 } },
];

/** Launch lands on the welcome screen; open the fixture project from the sidebar. */
async function openProject(page: Page) {
  await expect(page.getByRole('heading', { name: 'Welcome to πDesk' })).toBeVisible();
  await page.getByRole('button', { name: `Open ${project.displayName}` }).click();
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
}

async function installDesktopMock(page: Page, scenario: Scenario = {}) {
  const [firstTitle, secondTitle] = scenario.titles ?? ['Alpha', 'Beta'];
  await page.addInitScript(({ project, threads, openDelay, sendDelay, rememberedThreadId, assistantText, piInstalled, systemPiOnly, manualInstall, missingInstallPlan, deferEvents, catalogue, history }) => {
    let defaults: { provider?: string; modelId?: string; thinkingLevel?: string } = catalogue.length ? { provider: 'opencode-go', modelId: 'kimi-k2.6' } : {};
    localStorage.setItem('lastProject', project.id);
    if (rememberedThreadId) localStorage.setItem('lastThread', rememberedThreadId);
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    let nextCallback = 0;
    let nextListener = 0;
    let installed = piInstalled;
    let internPlans: any[] = [];
    (window as any).__git = {};
    let mcpServers: Record<string, any> = {};
    let mcpApprove = false;
    const sharedMcp: Record<string, any> = {
      'chrome-devtools': { command: 'npx', args: ['-y', 'chrome-devtools-mcp@latest'] },
      context7: { url: 'https://mcp.context7.com/mcp' },
      'eureka-db': { url: 'https://eureka.example/mcp', auth: 'oauth' },
    };
    const mcpView = (name: string, config: any, full: boolean) => ({
      name, transport: config.command ? 'stdio' : 'http', target: config.command ? [config.command, ...(config.args ?? [])].join(' ') : config.url,
      disabled: config.disabled === true, auth: config.auth, lifecycle: config.lifecycle, hasSecrets: !!(config.env || config.headers), ...(full ? { config } : {}),
    });
    let extensions: any[] = [{ source: 'npm:@narumitw/pi-usage', name: '@narumitw/pi-usage', kind: 'npm', version: '0.60.0', description: 'Usage stats for Pi', latest: '0.61.1', updateAvailable: true }];
    let finishInstall: ((error?: string) => void) | undefined;
    let connectEvents: (() => void) | undefined;
    let changedFiles = [{ path: 'sample.txt', status: 'M', additions: 1, deletions: 1, binary: false }];
    const clone = <T>(value: T): T => structuredClone(value);
    const emit = (payload: unknown) => {
      for (const [id, listener] of listeners) {
        if (listener.event === 'desktop-event') callbacks.get(listener.handler)?.({ event: listener.event, id, payload });
      }
    };
    const snapshot = (id: string) => {
      const row = threads.find(candidate => candidate.id === id);
      if (!row) throw new Error(`Unknown thread ${id}`);
      return {
        thread: clone(row),
        messages: history && id === 'a' ? clone(history) : [{ role: 'assistant', content: [{ type: 'text', text: assistantText ?? `History ${id}` }], timestamp: 1 }],
        state: { sessionId: row.sessionId, sessionFile: row.sessionFile, isStreaming: false,
          ...(catalogue.length ? { model: catalogue.find(model => model.provider === defaults.provider && model.id === defaults.modelId) ?? catalogue[0], thinkingLevel: defaults.thinkingLevel ?? 'medium' } : {}) },
        models: clone(catalogue), levels: catalogue.length ? ['off', 'medium', 'high'] : [], agents: [],
        commands: [{ name: 'mcp-auth', description: 'Authenticate with an MCP server', source: 'extension' }, { name: 'skill:mcp-scripting', description: 'Batch MCP calls in a script', source: 'skill' }],
        capabilities: {
          agents: false, nestedAgents: false, agentSteering: false, agentKill: false,
          agentRevive: false, planMode: false, permissions: false, modelSwitching: catalogue.length > 0,
          effortLevels: catalogue.length > 0, contextUsage: false, tokenUsage: false, worktrees: true,
        },
      };
    };
    const host = window as typeof window & {
      __TAURI_INTERNALS__?: unknown;
      __TAURI_EVENT_PLUGIN_INTERNALS__?: unknown;
      __mockDesktop?: unknown;
    };
    host.__TAURI_INTERNALS__ = {
      callbacks,
      transformCallback(callback: (event: unknown) => void) {
        const id = ++nextCallback;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback(id: number) { callbacks.delete(id); },
      async invoke(command: string, args: Record<string, unknown> = {}) {
        calls.push({ command, args: clone(args) });
        if (command === 'plugin:event|listen') {
          const id = ++nextListener;
          listeners.set(id, { event: String(args.event), handler: Number(args.handler) });
          if (deferEvents) return await new Promise<number>(resolve => { connectEvents = () => resolve(id); });
          return id;
        }
        if (command === 'plugin:event|unlisten') {
          listeners.delete(Number(args.eventId));
          return;
        }
        if (command === 'detect_harnesses') return installed ? [{ kind: 'pi', path: '/Users/test/.pidesk/runtime/node_modules/.bin/pi', version: '0.87.1', source: 'managed' }] : systemPiOnly ? [{ kind: 'pi', path: '/usr/local/bin/pi', version: '0.87.1', source: 'PATH' }] : [];
        if (command === 'harness_install_commands') return missingInstallPlan ? [] : [{ kind: 'pi', command: 'npm install --prefix /Users/test/.pidesk/runtime --ignore-scripts --no-audit --no-fund @earendil-works/pi-coding-agent', installPath: '/Users/test/.pidesk/runtime', agentDir: '/Users/test/.pidesk/agent', loginCommand: 'env -i HOME="$HOME" PATH="$PATH" PI_CODING_AGENT_DIR=/Users/test/.pidesk/agent /Users/test/.pidesk/runtime/node_modules/.bin/pi --no-approve' }];
        if (command === 'install_harness') {
          emit({ type: 'install_stage', kind: 'pi', stage: 'preparing' });
          emit({ type: 'install_progress', kind: 'pi', line: 'Checking Node.js and npm…' });
          if (manualInstall) return await new Promise<void>((resolve, reject) => {
            finishInstall = error => {
              emit({ type: 'install_finished', kind: 'pi', success: !error, error });
              if (error) reject(new Error(error));
              else { installed = true; resolve(); }
            };
          });
          installed = true;
          emit({ type: 'install_stage', kind: 'pi', stage: 'verifying' });
          emit({ type: 'install_progress', kind: 'pi', line: 'Private Pi verified.' });
          emit({ type: 'install_finished', kind: 'pi', success: true });
          return;
        }
        if (command === 'intern_snapshot') {
          if (!installed) throw new Error('Private Pi is missing. Install Pi in Settings.');
          const view = snapshot('a');
          return { ...view, thread: { ...view.thread, id: 'pidesk-intern', title: 'Pi Intern' }, messages: [{role:'assistant',content:[{type:'text',text:'I can inspect Pi or coordinate your project. Changes require approval.'}]}] };
        }
        if (command === 'intern_clear') {
          internPlans = internPlans.filter(plan => plan.threadId !== 'pidesk-intern');
          emit({type:'intern_changed'});
          const view = snapshot('a');
          return { ...view, thread: { ...view.thread, id: 'pidesk-intern', title: 'Pi Intern', sessionId: 'fresh' }, messages: [] };
        }
        if (command === 'intern_plans') return clone(internPlans);
        if (command === 'intern_prompt') {
          emit({type:'rpc',threadId:'pidesk-intern',frame:{type:'agent_start'}});
          emit({type:'rpc',threadId:'pidesk-intern',frame:{type:'message_end',message:{role:'user',content:String(args.message),timestamp:10}}});
          emit({type:'rpc',threadId:'pidesk-intern',frame:{type:'message_end',message:{role:'assistant',content:[{type:'text',text:'I inspected the request. I will propose an exact plan before changing anything.'}],timestamp:11}}});
          emit({type:'rpc',threadId:'pidesk-intern',frame:{type:'agent_settled'}});
          return;
        }
        if (command === 'intern_approve') {
          const plan = internPlans.find(plan => plan.id === args.planId);
          if (!plan) throw new Error('Plan expired');
          internPlans = internPlans.filter(candidate => candidate.id !== plan.id);
          if (args.approved) for (const action of plan.actions) if (action.kind === 'create_thread') threads.push({...threads[0],id:'intern-worker',title:action.title});
          emit({type:'intern_changed'});
          return;
        }
        if (command === 'intern_stop') {
          internPlans = [];
          emit({type:'intern_changed'});
          emit({type:'exited',threadId:'pidesk-intern',expected:true,stderr:''});
          return;
        }
        if (command === 'create_thread') {
          // A real new thread has no Pi session until Pi starts.
          const row = { ...threads[0], id: 'new-thread', title: 'New thread', harness: String(args.harness), sessionId: '', sessionFile: '' };
          threads.push(row);
          return clone(row);
        }
        if (command === 'list_projects') return [clone(project)];
        if (command === 'list_threads') return clone(threads);
        if (command === 'list_recent_threads') return clone(threads.filter(row => !row.archived).sort((x, y) => y.lastViewedAt.localeCompare(x.lastViewedAt)).slice(0, Number(args.limit)));
        if (command === 'open_thread' || command === 'restart_thread') {
          const id = String(args.threadId);
          await new Promise(resolve => setTimeout(resolve, openDelay[id] ?? 0));
          return snapshot(id);
        }
        if (command === 'send_prompt') {
          await new Promise(resolve => setTimeout(resolve, sendDelay));
          return;
        }
        if (command === 'set_thread_flags') {
          const row = threads.find(candidate => candidate.id === args.threadId);
          if (!row) throw new Error('Unknown thread');
          if (args.pinned !== undefined) row.pinned = Boolean(args.pinned);
          if (args.archived !== undefined) row.archived = Boolean(args.archived);
          return clone(row);
        }
        if (command === 'thread_delete_preview') {
          const row = threads.find(candidate => candidate.id === args.threadId);
          if (!row) throw new Error('Unknown thread');
          return { hasSession: Boolean(row.sessionFile), worktreePath: row.worktreePath,
            changedFiles: row.worktreePath ? ((window as any).__dirtyWorktreeFiles ?? 0) : 0 };
        }
        if (command === 'delete_thread') {
          const index = threads.findIndex(candidate => candidate.id === args.threadId);
          if (index < 0) throw new Error('Unknown thread');
          if (threads[index].worktreePath && (window as any).__dirtyWorktreeFiles && !args.discardChanges) throw new Error('Uncommitted files');
          threads.splice(index, 1);
          return;
        }
        if (command === 'rename_thread') {
          const row = threads.find(candidate => candidate.id === args.threadId);
          if (!row) throw new Error('Unknown thread to rename');
          row.title = String(args.title);
          return clone(row);
        }
        if (command === 'git_status') {
          const branch = 'branch' in (window as any).__git ? (window as any).__git.branch : 'main';
          return { isRepo: true, branch: branch ?? undefined, files: clone(changedFiles),
            additions: changedFiles.reduce((sum, file) => sum + file.additions, 0),
            deletions: changedFiles.reduce((sum, file) => sum + file.deletions, 0) };
        }
        if (command === 'git_file') {
          return { path: String(args.path), old: 'before\n', current: 'after\n',
            currentHash: 'expected-hash', binary: false, tooLarge: false };
        }
        if (command === 'git_revert_file') { changedFiles = []; return; }
        if (command === 'get_runtime_stats') {
          const runtime = (window as any).__runtimeStats ?? { appBytes: 90 * 1024 * 1024, threads: [] };
          const stopped: string[] = (window as any).__stoppedThreads ?? [];
          return { ...runtime, threads: runtime.threads.filter((thread: { threadId: string }) => !stopped.includes(thread.threadId)) };
        }
        if (command === 'stop_thread') {
          (window as any).__stoppedThreads = [...((window as any).__stoppedThreads ?? []), String(args.threadId)];
          return;
        }
        if (command === 'get_model_defaults') return clone(defaults);
        if (command === 'set_default_model') { defaults = { ...defaults, provider: String(args.provider), modelId: String(args.modelId) }; return clone(defaults); }
        if (command === 'set_default_thinking_level') { defaults = { ...defaults, thinkingLevel: String(args.level) }; return clone(defaults); }
        if (command === 'set_thread_model') {
          const model = catalogue.find(candidate => candidate.provider === args.provider && candidate.id === args.modelId);
          if (!model) throw new Error('Model not found');
          return { sessionId: 'session', isStreaming: false, model: clone(model), thinkingLevel: 'medium' };
        }
        if (command === 'terminal_open') {
          const channel = (args.output as { id: number }).id;
          setTimeout(() => callbacks.get(channel)?.({ index: 0, message: new TextEncoder().encode('Pi shell ready\r\n').buffer }), 20);
          return 'mock-terminal-1';
        }
        if (command === 'terminal_write' || command === 'terminal_ack' || command === 'terminal_resize' || command === 'terminal_close' || command === 'plugin:opener|reveal_item_in_dir') return;
        if (command === 'extensions_installed') return clone(extensions.map(({ latest, updateAvailable, ...row }) => ({ ...row, updateAvailable: false })));
        if (command === 'extensions_check_updates') return (window as any).__npmDown ? clone(extensions.map(({ latest, updateAvailable, ...row }) => ({ ...row, updateAvailable: false }))) : clone(extensions);
        if (command === 'extensions_catalog') {
          const all = [
            { name: 'pi-mcp-adapter', description: 'MCP adapter extension for Pi', author: 'nicopreme', downloadsLabel: '1M/mo', downloads: 1037931, publishedMs: Date.now() - 3 * 86400000, types: ['extension'], source: 'npm:pi-mcp-adapter' },
            { name: 'pi-web-access', description: 'Web search and URL fetching for Pi', author: 'nicopreme', downloadsLabel: '300K/mo', downloads: 300000, publishedMs: Date.now() - 86400000, types: ['extension', 'skill'], source: 'npm:pi-web-access' },
          ];
          const q = String(args.query).toLowerCase();
          return { packages: all.filter(item => !q || item.name.includes(q) || item.description.toLowerCase().includes(q)), page: Number(args.page), hasMore: false };
        }
        if (command === 'extensions_install') {
          const source = String(args.source).replace(/^pi install /, '');
          const name = source.replace(/^npm:/, '');
          extensions.push({ source: `npm:${name}`, name, kind: 'npm', version: '1.0.0', latest: '1.0.0', updateAvailable: false });
          return `npm:${name}`;
        }
        if (command === 'extensions_update') {
          const row = extensions.find(item => item.source === args.source);
          if (row) { row.version = row.latest; row.updateAvailable = false; }
          return;
        }
        if (command === 'extensions_remove') { extensions = extensions.filter(item => item.source !== args.source); return; }
        if (command === 'mcp_overview') return clone({
          adapterInstalled: true, adapterVersion: '2.37.0', path: '~/.pidesk/agent/mcp.json',
          servers: Object.entries(mcpServers).map(([name, config]) => mcpView(name, config, true)),
          approveTools: mcpApprove ? 'all' : 'off', raw: JSON.stringify({ mcpServers }, null, 2), hasComments: false,
          importSources: [{ id: 'config-mcp', path: '~/.config/mcp/mcp.json', servers: Object.entries(sharedMcp).map(([name, config]) => mcpView(name, config, false)) }],
        });
        if (command === 'mcp_import') { const copied = (args.names as string[]).filter(name => !mcpServers[name]); for (const name of copied) mcpServers[name] = clone(sharedMcp[name]); return copied; }
        if (command === 'mcp_save_server') { if (args.original && args.original !== args.name) delete mcpServers[String(args.original)]; mcpServers[String(args.name)] = clone(args.config); return; }
        if (command === 'mcp_set_enabled') { if (args.enabled) delete mcpServers[String(args.name)].disabled; else mcpServers[String(args.name)].disabled = true; return; }
        if (command === 'mcp_remove_server') { delete mcpServers[String(args.name)]; return; }
        if (command === 'mcp_set_approve_tools') { mcpApprove = Boolean(args.all); return; }
        if (command === 'compact_thread') return;
        if (command === 'list_thread_files') return { files: ['README.md', 'src/App.svelte', 'src/app.css', 'src/lib/api.ts', 'src/lib/components/conversation/Composer.svelte', 'docs/modes.md'], truncated: false };
        if (command === 'get_usage') return { tokens: { input: 1200, output: 300, cacheRead: 0, cacheWrite: 0, total: 1500 }, cost: 0.0123, contextUsage: { tokens: 1500, contextWindow: 200000, percent: 1 } };
        if (command === 'set_thread_mode') {
          const row = threads.find(candidate => candidate.id === args.threadId);
          if (!row) throw new Error('Unknown thread');
          row.mode = String(args.mode);
          return clone(row);
        }
        if (command === 'respond_ui' || command === 'abort_thread' || command === 'prewarm_thread') return;
        throw new Error(`Unexpected Tauri command: ${command}`);
      },
    };
    host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    host.__mockDesktop = {
      calls,
      emit,
      finishInstall(error?: string) { finishInstall?.(error); },
      connectEvents() { connectEvents?.(); },
      proposeInternPlan(plan: any) { internPlans.push(plan); emit({type:'intern_changed'}); },
      setWorktree(id: string, changedFiles: number) {
        const row = threads.find(candidate => candidate.id === id);
        if (row) row.worktreePath = `/Users/test/.pidesk/worktrees/${id}`;
        (window as any).__dirtyWorktreeFiles = changedFiles;
      },
    };
  }, {
    project,
    threads: [
      thread('a', firstTitle, '2026-01-03T00:00:00Z'),
      thread('b', secondTitle, '2026-01-02T00:00:00Z'),
    ],
    openDelay: scenario.openDelay ?? {},
    sendDelay: scenario.sendDelay ?? 0,
    rememberedThreadId: scenario.rememberedThreadId,
    assistantText: scenario.assistantText,
    piInstalled: scenario.piInstalled ?? true,
    systemPiOnly: scenario.systemPiOnly ?? false,
    manualInstall: scenario.manualInstall ?? false,
    missingInstallPlan: scenario.missingInstallPlan ?? false,
    deferEvents: scenario.deferEvents ?? false,
    catalogue: scenario.models ? MODEL_CATALOGUE : [],
    history: scenario.history,
  });
}

test('Pi Intern opens above resources, accepts screenshots, and does not stop when hidden', async ({ page }) => {
  await installDesktopMock(page, { models: true });
  await page.goto('/');
  await openProject(page);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.some((call: any) => call.command === 'intern_snapshot'))).toBe(false);
  await page.getByRole('button', {name:'Ask Pi Intern'}).click();
  const chat = page.getByRole('dialog', {name:'Pi Intern',exact:true});
  await expect(chat).toBeVisible();
  await expect(chat).toContainText('Changes require approval');
  // Intern is app-wide: the open sidebar project is not attached implicitly.
  await expect(chat.locator('header')).toContainText('All of πDesk');
  await chat.getByRole('button', {name:'Attach project'}).click();
  await chat.getByRole('menuitemradio', {name:'desktop-e2e'}).click();
  await expect(chat.getByRole('button', {name:'Detach desktop-e2e'})).toBeVisible();
  const image = await page.screenshot();
  await chat.locator('input[type="file"]').setInputFiles({name:'issue.png',mimeType:'image/png',buffer:image});
  await expect(chat.getByAltText('issue.png')).toBeVisible();
  await chat.getByRole('textbox', {name:'Message Pi Intern'}).fill('Explain this screenshot');
  await chat.getByRole('button', {name:'Send to Pi Intern'}).click();
  await expect(chat).toContainText('I inspected the request');
  const prompt = await page.evaluate(() => (window as any).__mockDesktop.calls.find((call: any) => call.command === 'intern_prompt').args);
  expect(prompt.projectId).toBe('project-1');
  expect(prompt.images).toHaveLength(1);
  expect(prompt.images[0].mimeType).toBe('image/jpeg');
  await chat.getByRole('button', {name:'Detach desktop-e2e'}).click();
  await expect(chat.locator('header')).toContainText('All of πDesk');
  await chat.getByRole('button', {name:'Hide Pi Intern'}).click();
  await expect(chat).toBeHidden();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.some((call: any) => call.command === 'intern_stop'))).toBe(false);
  await page.getByRole('button', {name:'Ask Pi Intern'}).click();
  await expect(chat).toContainText('I inspected the request');
});

test('opening Intern from a thread attaches that thread\'s project', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.getByRole('button', {name:'Ask Pi Intern'}).click();
  const chat = page.getByRole('dialog', {name:'Pi Intern',exact:true});
  await expect(chat.locator('header')).toContainText('desktop-e2e');
  await chat.getByRole('button', {name:'Detach desktop-e2e'}).click();
  await expect(chat.locator('header')).toContainText('All of πDesk');
  await chat.getByRole('button', {name:'Hide Pi Intern'}).click();
  await page.getByRole('button', {name:'Ask Pi Intern'}).click();
  await expect(chat.getByRole('button', {name:'Detach desktop-e2e'})).toBeVisible();
  await chat.getByRole('textbox', {name:'Message Pi Intern'}).fill('What changed here?');
  await chat.getByRole('button', {name:'Send to Pi Intern'}).click();
  await expect(chat).toContainText('I inspected the request');
  const prompt = await page.evaluate(() => (window as any).__mockDesktop.calls.find((call: any) => call.command === 'intern_prompt').args);
  expect(prompt.projectId).toBe('project-1');
});

test('clearing Intern starts a fresh conversation in place and drops its pending plans', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await page.getByRole('button', {name:'Ask Pi Intern'}).click();
  const chat = page.getByRole('dialog', {name:'Pi Intern',exact:true});
  await chat.getByRole('textbox', {name:'Message Pi Intern'}).fill('Do something');
  await chat.getByRole('button', {name:'Send to Pi Intern'}).click();
  await expect(chat).toContainText('I inspected the request');
  await page.evaluate(() => (window as any).__mockDesktop.proposeInternPlan({id:'plan-c',threadId:'pidesk-intern',threadTitle:'Pi Intern',summary:'Coordinator plan',actions:[{kind:'install_pi'}],details:[{}],executing:false}));
  await expect(chat).toContainText('Coordinator plan');
  await chat.getByRole('textbox', {name:'Message Pi Intern'}).fill('draft stays');
  await chat.getByRole('button', {name:'Clear Pi Intern conversation'}).click();
  await expect(chat).not.toContainText('I inspected the request');
  await expect(chat).not.toContainText('Coordinator plan');
  await expect(chat.getByRole('textbox', {name:'Message Pi Intern'})).toHaveValue('draft stays');
  await expect(chat.getByRole('textbox', {name:'Message Pi Intern'})).toBeEnabled();
  const calls = await page.evaluate(() => (window as any).__mockDesktop.calls.map((call: any) => call.command));
  expect(calls).toContain('intern_clear');
  expect(calls).not.toContain('intern_stop');
});

test('Intern batches require explicit approval and started threads are ordinary project threads', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.getByRole('button', {name:'Ask Pi Intern'}).click();
  const chat = page.getByRole('dialog', {name:'Pi Intern',exact:true});
  await expect(chat.getByRole('textbox', {name:'Message Pi Intern'})).toBeEnabled();
  await page.evaluate(() => (window as any).__mockDesktop.proposeInternPlan({id:'plan-1',threadId:'pidesk-intern',threadTitle:'Pi Intern',projectPath:'/tmp/pidesk-e2e',summary:'Start a project thread',actions:[{kind:'create_thread',projectId:'project-1',title:'Investigate screenshot',message:'Investigate only'}],details:[{}],executing:false}));
  await expect(chat).toContainText('Start a project thread');
  await expect(page.locator('.thread-link[title="Investigate screenshot"]')).toHaveCount(0);
  await chat.getByRole('button', {name:'Approve exact plan'}).click();
  await expect(page.locator('.thread-link[title="Investigate screenshot"]')).toBeVisible();
  await page.evaluate(() => (window as any).__mockDesktop.proposeInternPlan({id:'plan-2',threadId:'pidesk-intern',threadTitle:'Pi Intern',projectPath:'/tmp/pidesk-e2e',summary:'Intern proposes a file change',actions:[{kind:'write_file',scope:'project',path:'sample.txt',expected:'before',content:'after'}],details:[{}],executing:false}));
  await expect(chat).toContainText('Intern proposes a file change');
  await chat.getByRole('button', {name:'Reject plan'}).click();
  await expect(chat.getByText('Intern proposes a file change')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call:any) => call.command==='intern_approve').map((call:any)=>call.args))).toEqual([{planId:'plan-1',approved:true},{planId:'plan-2',approved:false}]);
  await chat.getByRole('button', {name:'Stop Intern', exact:true}).click();
  await expect(chat).toContainText('Intern stopped');
});

test('private Pi setup ignores system Pi and never auto-installs', async ({ page }, testInfo) => {
  await installDesktopMock(page, { piInstalled: false, systemPiOnly: true });
  await page.goto('/');
  await expect(page).toHaveTitle('πDesk');
  await expect(page.getByRole('heading', { name: 'A Pi of its own.' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Install Pi', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: /Locate|Choose…/ })).toHaveCount(0);
  await expect(page.getByText('/Users/test/.pidesk/runtime', { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.some((call: any) => call.command === 'install_harness'))).toBe(false);
  await page.screenshot({ path: testInfo.outputPath('private-pi-setup.png') });
  await page.getByRole('button', { name: 'Install Pi', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Your Pi is ready.' })).toBeVisible();
  await expect(page.getByRole('log', { name: 'Pi installation output' })).toContainText('Private Pi verified.');
  await expect(page.getByRole('textbox', { name: 'Private Pi sign-in command' })).toHaveValue(/PI_CODING_AGENT_DIR=.*\.pidesk\/agent/);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: any) => call.command === 'install_harness'))).toEqual([{ command: 'install_harness', args: { kind: 'pi' } }]);
  await page.getByRole('button', { name: 'Continue to πDesk' }).click();
  await openProject(page);
});

test('Install waits for its live event subscription before enabling', async ({ page }) => {
  await installDesktopMock(page, { piInstalled: false, deferEvents: true });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'A Pi of its own.' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Install Pi', exact: true })).toBeDisabled();
  await page.evaluate(() => (window as any).__mockDesktop.connectEvents());
  await expect(page.getByRole('button', { name: 'Install Pi', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Install Pi', exact: true }).click();
  await expect(page.getByRole('log', { name: 'Pi installation output' })).toContainText('Checking Node.js and npm…');
  await expect(page.getByRole('heading', { name: 'Your Pi is ready.' })).toBeVisible();
});

test('installer streams actual stages, retains errors, and supports retry', async ({ page }) => {
  await installDesktopMock(page, { piInstalled: false, manualInstall: true });
  await page.goto('/');
  await page.getByRole('button', { name: 'Install Pi', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Checking requirements…', exact: true })).toBeDisabled();
  const log = page.getByRole('log', { name: 'Pi installation output' });
  await expect(log).toContainText('Checking Node.js and npm…');
  await page.evaluate(() => {
    const mock = (window as any).__mockDesktop;
    mock.emit({ type: 'install_stage', kind: 'pi', stage: 'installing' });
    mock.emit({ type: 'install_progress', kind: 'pi', line: 'Fetching packages <script>not HTML</script>' });
  });
  await expect(page.getByRole('button', { name: 'Installing packages…', exact: true })).toBeDisabled();
  await expect(log).toContainText('Fetching packages <script>not HTML</script>');
  await expect(log.locator('script')).toHaveCount(0);
  await page.evaluate(() => (window as any).__mockDesktop.finishInstall('Registry unavailable. Check your connection.'));
  await expect(page.getByRole('alert')).toContainText('Registry unavailable.');
  await expect(log).toContainText('Fetching packages');
  await page.getByRole('button', { name: 'Retry installation' }).click();
  await expect(page.getByRole('button', { name: 'Checking requirements…', exact: true })).toBeDisabled();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await page.evaluate(() => {
    const mock = (window as any).__mockDesktop;
    mock.emit({ type: 'install_stage', kind: 'pi', stage: 'verifying' });
    mock.emit({ type: 'install_progress', kind: 'pi', line: 'Checking private Pi version…' });
  });
  await expect(page.getByRole('button', { name: 'Verifying installation…', exact: true })).toBeDisabled();
  await page.evaluate(() => (window as any).__mockDesktop.finishInstall());
  await expect(page.getByRole('heading', { name: 'Your Pi is ready.' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: any) => call.command === 'install_harness'))).toHaveLength(2);
});

test('live installer output stays bounded during a noisy install', async ({ page }) => {
  await installDesktopMock(page, { piInstalled: false, manualInstall: true });
  await page.goto('/');
  await page.getByRole('button', { name: 'Install Pi', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Checking requirements…', exact: true })).toBeDisabled();
  await page.evaluate(() => {
    const mock = (window as any).__mockDesktop;
    for (let i = 0; i < 300; i++) mock.emit({ type: 'install_progress', kind: 'pi', line: `package-${i} ` + 'x'.repeat(5000) });
  });
  const output = await page.getByRole('log', { name: 'Pi installation output' }).innerText();
  expect(output).not.toContain('package-99 ');
  expect(output).toContain('package-100 ');
  expect(output).toContain('package-299 ');
  expect(output.length).toBeLessThan(200 * 4100);
  await page.evaluate(() => (window as any).__mockDesktop.finishInstall());
  await expect(page.getByRole('heading', { name: 'Your Pi is ready.' })).toBeVisible();
});

test('missing private installer metadata fails closed instead of using a global fallback', async ({ page }) => {
  await installDesktopMock(page, { piInstalled: false, missingInstallPlan: true });
  await page.goto('/');
  await expect(page.getByRole('alert')).toContainText('Private installer information is unavailable.');
  await expect(page.getByRole('button', { name: 'Install Pi', exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.some((call: any) => call.command === 'install_harness'))).toBe(false);
});

test('new threads and settings are Pi-only', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await expect(page.getByRole('combobox', { name: 'Harness for new threads' })).toHaveCount(0);
  await page.locator('.new-thread').click();
  await expect(page.getByText('History new-thread')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: any) => call.command === 'create_thread'))).toEqual([{ command: 'create_thread', args: { projectId: 'project-1', harness: 'pi', isolated: true } }]);
  await expect(page.getByRole('button', { name: 'Toggle agents' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await page.getByRole('dialog', { name: 'Settings' }).getByRole('button', { name: 'Pi runtime' }).click();
  await expect(page.getByRole('heading', { name: 'Pi runtime' })).toBeVisible();
  await expect(page.getByText('/Users/test/.pidesk/runtime/node_modules/.bin/pi', { exact: true })).toBeVisible();
  await expect(page.getByText('OMP', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Private Pi sign-in command' })).toBeVisible();
});

test('extension updates badge Settings, and packages install, update, and come from pi.dev', async ({ page }, testInfo) => {
  await installDesktopMock(page);
  await page.goto('/');
  // The startup check finds an update and badges the Settings gear.
  await expect(page.locator('.gear-badge')).toBeVisible();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await expect(settings.getByRole('heading', { name: 'Extensions' })).toBeVisible();
  await expect(settings.getByRole('button', { name: /Extensions/ }).locator('.nav-badge')).toHaveText('1');
  const row = settings.getByRole('listitem').filter({ hasText: '@narumitw/pi-usage' });
  await expect(row).toContainText('Update available: v0.60.0 → v0.61.1. Please update it.');
  await page.waitForTimeout(300);
  await page.screenshot({ path: testInfo.outputPath('extensions-installed.png') });
  await row.getByRole('button', { name: 'Update to v0.61.1' }).click();
  await expect(row).toContainText('Up to date');
  await expect(settings.locator('.nav-badge')).toHaveCount(0);
  await expect(page.locator('.gear-badge')).toHaveCount(0);

  // Paste a full install command.
  await settings.getByLabel('Install a package').fill('pi install npm:pi-goal-x');
  await settings.getByRole('button', { name: 'Install', exact: true }).click();
  await expect(settings.getByRole('status')).toContainText('Installed pi-goal-x');
  await expect(settings.getByRole('listitem').filter({ hasText: 'pi-goal-x' })).toBeVisible();

  // Discover searches the pi.dev gallery and installs from a card.
  await settings.getByRole('tab', { name: 'Discover' }).click();
  await expect(settings.locator('article', { hasText: 'pi-mcp-adapter' })).toContainText('1M/mo');
  await page.screenshot({ path: testInfo.outputPath('extensions-discover.png') });
  await settings.getByRole('searchbox', { name: 'Search packages' }).fill('web');
  await expect(settings.locator('article')).toHaveCount(1);
  await settings.locator('article', { hasText: 'pi-web-access' }).getByRole('button', { name: 'Install' }).click();
  await expect(settings.locator('article', { hasText: 'pi-web-access' })).toContainText('Installed');
  // Reopening shows the same per-package results without asking npm again.
  const checks = await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'extensions_check_updates').length);
  await settings.getByRole('button', { name: 'Close settings' }).click();
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await settings.getByRole('button', { name: /Extensions/ }).click();
  await expect(settings.getByRole('listitem').filter({ hasText: '@narumitw/pi-usage' })).toContainText('Up to date');
  await expect(settings.getByText(/Up to date · checked/)).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'extensions_check_updates').length)).toBe(checks);
  const calls = await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => ['extensions_install', 'extensions_update'].includes(call.command)).map((call: { command: string; args: unknown }) => [call.command, call.args]));
  expect(calls).toEqual([
    ['extensions_update', { source: 'npm:@narumitw/pi-usage' }],
    ['extensions_install', { source: 'pi install npm:pi-goal-x' }],
    ['extensions_install', { source: 'npm:pi-web-access' }],
  ]);
});

test('when npm cannot be reached, Extensions says so instead of claiming up to date', async ({ page }) => {
  await installDesktopMock(page);
  await page.addInitScript(() => { (window as any).__npmDown = true; });
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await settings.getByRole('button', { name: /Extensions/ }).click();
  await expect(settings.getByText(/Couldn't reach npm for 1 package/)).toBeVisible();
  await expect(settings.getByRole('listitem').filter({ hasText: '@narumitw/pi-usage' })).toContainText("Couldn't check");
  await expect(settings.getByText(/Up to date/)).toHaveCount(0);
});

test('MCP servers: copy chosen servers from the shared file, add one by URL, edit, and turn off', async ({ page }, testInfo) => {
  await installDesktopMock(page);
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await settings.getByRole('button', { name: 'MCP servers' }).click();
  await expect(settings.getByRole('heading', { name: 'MCP servers' })).toBeVisible();
  await expect(settings.getByText('3 servers in ~/.config/mcp/mcp.json')).toBeVisible();
  await settings.getByRole('button', { name: 'Choose servers…' }).click();
  const copy = settings.getByRole('group', { name: 'Copy MCP servers' });
  await copy.getByRole('checkbox').first().uncheck();
  await copy.getByRole('button', { name: 'Copy 2 servers' }).click();
  await expect(settings.getByRole('status')).toContainText('Copied context7, eureka-db into πDesk');
  const list = settings.getByRole('list', { name: 'MCP servers' });
  await expect(list.getByRole('listitem')).toHaveCount(2);
  await expect(settings.getByText(/eureka-db needs a one-time sign-in/)).toBeVisible();
  await settings.getByRole('button', { name: 'Sign in to eureka-db' }).click();
  const terminal = page.getByRole('dialog', { name: 'Pi terminal' });
  await expect(terminal).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'terminal_write').map((call: { args: { data: string } }) => call.args.data).join(''))).toBe("pi '/mcp-auth eureka-db'\r");
  await terminal.getByRole('button', { name: 'Close terminal' }).click();
  // The remaining shared server is still offered.
  await expect(settings.getByText('1 server in ~/.config/mcp/mcp.json')).toBeVisible();

  await settings.getByRole('button', { name: 'Add server' }).click();
  const form = settings.getByRole('form', { name: 'Add MCP server' });
  await form.getByLabel('Name', { exact: true }).fill('deepwiki');
  await form.getByLabel('URL', { exact: true }).fill('https://mcp.deepwiki.com/mcp');
  await form.getByRole('button', { name: 'Add server' }).click();
  await expect(list.getByRole('listitem')).toHaveCount(3);
  await page.screenshot({ path: testInfo.outputPath('mcp-servers.png') });

  await list.getByRole('button', { name: 'Edit deepwiki' }).click();
  const editForm = settings.getByRole('form', { name: 'Edit deepwiki' });
  await editForm.getByLabel('Headers').fill('Authorization: Bearer ${DEEPWIKI_TOKEN}');
  await editForm.getByRole('button', { name: 'Save' }).click();
  await expect(list.getByRole('listitem').filter({ hasText: 'deepwiki' })).toContainText('keys');

  await list.getByRole('switch', { name: 'context7 enabled' }).uncheck();
  await expect(list.getByRole('listitem').filter({ hasText: 'context7' })).toContainText('Off');
  await settings.getByRole('switch', { name: 'Ask before every MCP tool call' }).check();

  const calls = await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command.startsWith('mcp_') && call.command !== 'mcp_overview').map((call: { command: string; args: unknown }) => [call.command, call.args]));
  expect(calls).toEqual([
    ['mcp_import', { source: 'config-mcp', names: ['context7', 'eureka-db'] }],
    ['mcp_save_server', { original: null, name: 'deepwiki', config: { url: 'https://mcp.deepwiki.com/mcp' } }],
    ['mcp_save_server', { original: 'deepwiki', name: 'deepwiki', config: { url: 'https://mcp.deepwiki.com/mcp', headers: { Authorization: 'Bearer ${DEEPWIKI_TOKEN}' } } }],
    ['mcp_set_enabled', { name: 'context7', enabled: false }],
    ['mcp_set_approve_tools', { all: true }],
  ]);
});

test('Settings shows full private paths and opens and closes the Pi terminal', async ({ page }) => {
  await installDesktopMock(page);
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await settings.getByRole('button', { name: 'Pi runtime' }).click();
  await expect(settings.locator('.install-paths')).toContainText('/Users/test/.pidesk/runtime/node_modules/.bin/pi');
  await expect(settings.locator('.install-paths')).toContainText('/Users/test/.pidesk/runtime');
  await expect(settings.locator('.install-paths')).toContainText('/Users/test/.pidesk/agent');
  await settings.getByRole('button', { name: 'Copy path' }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe('/Users/test/.pidesk/runtime/node_modules/.bin/pi');
  await settings.getByRole('button', { name: 'Reveal in Finder' }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.some((call: { command: string }) => call.command === 'plugin:opener|reveal_item_in_dir'))).toBe(true);
  await settings.locator('.install-actions').getByRole('button', { name: 'Open terminal' }).click();
  const terminal = page.getByRole('dialog', { name: 'Pi terminal' });
  await expect(terminal).toBeVisible();
  await expect(terminal.locator('.xterm-screen')).toContainText('Pi shell ready');
  // Rendered output is acknowledged back to the backend for flow control.
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'terminal_ack').reduce((sum: number, call: { args: { bytes: number } }) => sum + call.args.bytes, 0))).toBe(16);
  // Typed input reaches the backend in order.
  await terminal.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type('pi --version', { delay: 0 });
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'terminal_write').map((call: { args: { data: string } }) => call.args.data).join(''))).toBe('pi --version');
  await terminal.getByRole('button', { name: 'Close terminal' }).click();
  await expect(terminal).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.some((call: { command: string }) => call.command === 'terminal_close'))).toBe(true);
});

test('terminal opens from setup and the command switcher', async ({ page }) => {
  await installDesktopMock(page, { piInstalled: false });
  await page.goto('/');
  await page.getByRole('button', { name: 'Install Pi', exact: true }).click();
  await page.getByRole('button', { name: 'Open terminal' }).click();
  await expect(page.getByRole('dialog', { name: 'Pi terminal' })).toBeVisible();
  await page.getByRole('button', { name: 'Close terminal' }).click();
  await page.getByRole('button', { name: 'Continue to πDesk' }).click();
  await page.keyboard.press('Meta+k');
  await page.getByRole('button', { name: /Open Pi terminal/ }).click();
  await expect(page.getByRole('dialog', { name: 'Pi terminal' })).toBeVisible();
});

test('assistant HTML cannot carry layout CSS, scripts, or remote images', async ({ page }) => {
  await installDesktopMock(page, {
    assistantText:
      '<div style="position:fixed;inset:0;z-index:99999;background:#000">overlay</div><script>document.title = "XSS1"<\/script><img src="x" onerror="document.title = \'XSS2\'"><a href="javascript:document.title = \'XSS3\'">click</a>plain text',
  });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('plain text')).toBeVisible();
  await expect(page.getByText('overlay')).toBeVisible();

  const probe = await page.evaluate(() => {
    const assistant = document.querySelector('.assistant');
    const anchors = Array.from(assistant?.querySelectorAll('a') ?? []);
    return {
      styleAttributes: assistant?.querySelectorAll('[style]').length ?? -1,
      positioned: assistant?.querySelector('[style*="position"]') !== null,
      scripts: assistant?.querySelectorAll('script').length ?? -1,
      images: assistant?.querySelectorAll('img').length ?? -1,
      javascriptHrefs: anchors.filter((anchor) =>
        (anchor.getAttribute('href') ?? '').startsWith('javascript:'),
      ).length,
      title: document.title,
    };
  });
  expect(probe).toEqual({
    styleAttributes: 0,
    positioned: false,
    scripts: 0,
    images: 0,
    javascriptHrefs: 0,
    title: 'πDesk',
  });
});

test('startup does not spawn a harness and newer thread selection wins', async ({ page }) => {
  await installDesktopMock(page, { openDelay: { a: 200, b: 15 } });
  await page.goto('/');
  await openProject(page);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'open_thread'))).toHaveLength(0);

  await page.locator('.thread-link[title="Alpha"]').click();
  await page.locator('.thread-link[title="Beta"]').click();
  await expect(page.locator('.top-thread')).toHaveText('Beta');
  await expect(page.getByText('History b')).toBeVisible();
  await page.waitForTimeout(250);
  await expect(page.locator('.top-thread')).toHaveText('Beta');
  await expect(page.getByText('History a')).toHaveCount(0);
});

test('launch shows the welcome screen and reopens a last-used thread in one click', async ({ page }, testInfo) => {
  await installDesktopMock(page, { rememberedThreadId: 'a' });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Welcome to πDesk' })).toBeVisible();
  const projects = page.getByRole('region', { name: 'Your projects' });
  await expect(projects.getByRole('button', { name: /desktop-e2e/ })).toBeVisible();
  const lastUsed = page.getByRole('region', { name: 'Last used' });
  await expect(lastUsed.getByRole('button')).toHaveText([/Alpha/, /Beta/]);
  await page.screenshot({ path: testInfo.outputPath('welcome.png') });
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'open_thread' || call.command === 'list_threads'))).toHaveLength(0);

  await lastUsed.getByRole('button', { name: /Alpha/ }).click();
  await expect(page.locator('.thread-row.active .thread-title')).toHaveText('Alpha');
  await expect(page.getByText('History a')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'open_thread').map((call: { args: { threadId: string } }) => call.args.threadId))).toEqual(['a']);

  await page.getByRole('button', { name: 'Welcome', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Welcome to πDesk' })).toBeVisible();
  await projects.getByRole('button', { name: /desktop-e2e/ }).click();
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
});

test('thread popover closes after actions and rename saves or cancels in place', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.getByRole('button', { name: 'Actions for Alpha' }).click();
  await page.getByRole('menuitem', { name: 'Pin', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: 'Pin', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Actions for Alpha' }).click();
  await page.getByRole('menuitem', { name: 'Unpin' }).click();
  await page.locator('.thread-link[title="Alpha"]').click({ button: 'right' });
  await expect(page.getByRole('menuitem', { name: 'Archive' })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Archive' }).click();
  await expect(page.getByRole('menuitem', { name: 'Archive' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Show archived' }).click();
  await page.getByRole('button', { name: 'Actions for Alpha' }).click();
  await page.getByRole('menuitem', { name: 'Unarchive' }).click();
  await page.getByRole('button', { name: 'Actions for Alpha' }).click();
  await page.getByRole('menuitem', { name: 'Rename' }).click();
  const input = page.getByRole('textbox', { name: 'Thread title' });
  await input.fill('Renamed Alpha');
  await input.press('Escape');
  await expect(page.locator('.thread-link[title="Alpha"]')).toBeVisible();
  await page.locator('.thread-link[title="Alpha"]').dblclick();
  await input.fill('Renamed Alpha');
  await input.press('Enter');
  await expect(page.locator('.thread-link[title="Renamed Alpha"]')).toBeVisible();
});

test('deleting a thread confirms safely and selects the next thread', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.getByRole('button', { name: 'Actions for Alpha' }).click();
  await page.getByRole('menuitem', { name: 'Delete thread…' }).click();
  const dialog = page.getByRole('alertdialog', { name: 'Delete thread?' });
  await expect(dialog).toContainText('Conversation history will be deleted');
  await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0);
  await expect(page.locator('.thread-link[title="Alpha"]')).toBeVisible();
  await page.getByRole('button', { name: 'Actions for Alpha' }).click();
  await page.getByRole('menuitem', { name: 'Delete thread…' }).click();
  await dialog.getByRole('button', { name: 'Delete thread', exact: true }).click();
  await expect(page.locator('.thread-link[title="Alpha"]')).toHaveCount(0);
  await expect(page.getByText('History b')).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem('lastThread'))).toBe('b');
});

test('dirty isolated thread requires explicit discard', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.evaluate(() => (window as any).__mockDesktop.setWorktree('a', 3));
  await page.getByRole('button', { name: 'Actions for Alpha' }).click();
  await page.getByRole('menuitem', { name: 'Delete thread…' }).click();
  const dialog = page.getByRole('alertdialog', { name: 'Delete thread?' });
  await expect(dialog).toContainText('3 uncommitted files will be discarded');
  await dialog.getByRole('button', { name: 'Delete thread and discard changes' }).click();
  await expect(page.locator('.thread-link[title="Alpha"]')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.find((call: { command: string }) => call.command === 'delete_thread')?.args)).toEqual({ threadId: 'a', discardChanges: true });
});

test('approval emitted while opening remains actionable', async ({ page }) => {
  await installDesktopMock(page, { openDelay: { a: 150 } });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await page.evaluate(() => (window as any).__mockDesktop.emit({
    type: 'rpc', threadId: 'a',
    frame: { type: 'extension_ui_request', id: 'approval-1', method: 'confirm',
      title: 'Approve change', message: 'May the agent edit this file?' },
  }));
  await expect(page.getByRole('group', { name: 'Request: Approve change' })).toBeVisible();
  await expect(page.getByText('May the agent edit this file?')).toBeVisible();
});

test('threads default to Auto and can switch to Plan from the composer', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  const modes = page.getByRole('radiogroup', { name: 'Agent mode' });
  await expect(modes.getByRole('radio', { name: 'Auto' })).toHaveAttribute('aria-checked', 'true');
  await modes.getByRole('radio', { name: 'Plan' }).click();
  await expect(modes.getByRole('radio', { name: 'Plan' })).toHaveAttribute('aria-checked', 'true');
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'set_thread_mode').map((call: { args: unknown }) => call.args))).toEqual([{ threadId: 'a', mode: 'plan' }]);
  // Mode is per thread and survives switching away and back.
  await page.locator('.thread-link[title="Beta"]').click();
  await expect(modes.getByRole('radio', { name: 'Auto' })).toHaveAttribute('aria-checked', 'true');
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(modes.getByRole('radio', { name: 'Plan' })).toHaveAttribute('aria-checked', 'true');
  // Host controls never show up as slash commands.
  await page.getByRole('combobox', { name: 'Message' }).fill('/pidesk');
  await expect(page.getByText('pidesk-mode')).toHaveCount(0);
});

const PLAN_FRAME = (id: string) => ({
  type: 'extension_ui_request', id, method: 'select',
  title: 'Run this plan in Auto mode?\n\n## Add dark mode toggle\n\n1. Edit `src/theme.ts`\n2. Run **bun test**',
  options: ['Approve and run in Auto', 'Revise with feedback', 'Decline'],
});

test('a plan opens in the side panel as rendered Markdown and can be approved', async ({ page }, testInfo) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.evaluate(frame => (window as any).__mockDesktop.emit({ type: 'rpc', threadId: 'a', frame }), PLAN_FRAME('plan-1'));
  const review = page.getByRole('dialog', { name: 'Plan review' });
  await expect(review.getByRole('heading', { name: 'Add dark mode toggle' })).toBeVisible();
  await expect(review.locator('code', { hasText: 'src/theme.ts' })).toBeVisible();
  await expect(review.locator('strong', { hasText: 'bun test' })).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('plan-review.png') });
  // Hiding keeps the plan pending behind a review bar.
  await review.getByRole('button', { name: 'Hide plan review' }).click();
  await expect(review).toHaveCount(0);
  await page.getByRole('button', { name: /Review plan/ }).click();
  await review.getByRole('button', { name: /Approve & run/ }).click();
  await expect(review).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Review plan/ })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'respond_ui').map((call: { args: unknown }) => call.args))).toEqual([{ threadId: 'a', requestId: 'plan-1', response: { value: 'Approve and run in Auto' } }]);
});

test('a pending plan turns the composer into an approval strip and flags the thread', async ({ page }, testInfo) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.evaluate(frame => (window as any).__mockDesktop.emit({ type: 'rpc', threadId: 'a', frame }), {
    ...PLAN_FRAME('plan-3'),
    title: 'Run this plan in Auto mode?\n\n1. Edit `src/theme.ts`\n2. Edit `src/app.css`\n3. Run `bun test`',
  });
  const strip = page.getByRole('group', { name: 'Plan approval' });
  await expect(strip).toContainText('Plan ready');
  await expect(strip).toContainText('3 steps');
  await expect(strip).toContainText('2 files');
  await expect(strip).toContainText('runs bun test');
  await expect(page.locator('.thread-row', { hasText: 'Alpha' }).locator('.row-pill')).toHaveText('Needs approval');
  await expect(page.locator('.status-pill')).toContainText('Needs approval');
  await page.getByRole('button', { name: 'Hide plan review' }).click();
  await page.waitForTimeout(500);
  await page.screenshot({ path: testInfo.outputPath('approval-strip.png') });
  await strip.getByRole('button', { name: /Approve & run/ }).click();
  await expect(strip).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'respond_ui').map((call: { args: unknown }) => call.args))).toEqual([{ threadId: 'a', requestId: 'plan-3', response: { value: 'Approve and run in Auto' } }]);
});

test('a Plan-mode permission request shows the exact command and can be allowed once', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.evaluate(() => (window as any).__mockDesktop.emit({ type: 'rpc', threadId: 'a', frame: {
    type: 'extension_ui_request', id: 'ask-1', method: 'select',
    title: 'Plan mode: allow this?\n\nRun in the shell:\nnpm test', options: ['Allow once', 'Allow for this run (Auto)', 'Keep blocked'],
  } }));
  const card = page.getByRole('group', { name: /Request: Plan mode: allow this\?/ });
  await expect(card).toContainText('npm test');
  await card.getByRole('button', { name: 'Allow once' }).click();
  await expect(card).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'respond_ui').map((call: { args: unknown }) => call.args))).toEqual([{ threadId: 'a', requestId: 'ask-1', response: { value: 'Allow once' } }]);
});

const MCP_RUN = [
  { role: 'user', content: 'use eureka mcp to find good ideas', timestamp: 1000 },
  { role: 'assistant', timestamp: 2000, content: [
    { type: 'thinking', thinking: 'The user wants to use the eureka MCP to find good ideas. Connect first.' },
    { type: 'toolCall', id: 'c1', name: 'mcp', arguments: { connect: 'eureka-db' } },
  ] },
  { role: 'toolResult', toolCallId: 'c1', toolName: 'mcp', content: [{ type: 'text', text: 'Connected to eureka-db (18 tools).' }], details: { mode: 'connect', server: 'eureka-db' }, timestamp: 3000 },
  { role: 'assistant', timestamp: 5000, content: [
    { type: 'thinking', thinking: 'Connected. Now search with filters.' },
    { type: 'toolCall', id: 'c2', name: 'eureka-db_get_saved_ideas', arguments: {} },
    { type: 'toolCall', id: 'c3', name: 'eureka-db_search_business_ideas', arguments: { min_opportunity_score: 80, max_difficulty: 3, has_revenue_signal: true } },
  ] },
  { role: 'toolResult', toolCallId: 'c2', toolName: 'eureka-db_get_saved_ideas', content: [{ type: 'text', text: '{"ideas":[{"id":1},{"id":2},{"id":3}]}' }], details: { server: 'eureka-db', tool: 'get_saved_ideas' }, timestamp: 8000 },
  { role: 'toolResult', toolCallId: 'c3', toolName: 'eureka-db_search_business_ideas', content: [{ type: 'text', text: JSON.stringify({ results: Array.from({ length: 21 }, (_, i) => ({ id: i })) }) }], details: { server: 'eureka-db', tool: 'search_business_ideas' }, timestamp: 12000 },
  { role: 'assistant', timestamp: 15000, content: [
    { type: 'thinking', thinking: 'I got 21 ideas. Let me narrow to the best with a script.' },
    { type: 'toolCall', id: 'c4', name: 'mcpScript', arguments: { code: '// top ideas\nconst r = await tools.eureka_db_search_business_ideas({ min_opportunity_score: 90 });\nemit(r.data);' } },
  ] },
  { role: 'toolResult', toolCallId: 'c4', toolName: 'mcpScript', content: [{ type: 'text', text: '[{"name":"AI bookkeeping for salons"}]' }], details: { mode: 'script' }, timestamp: 30000 },
  { role: 'assistant', timestamp: 43000, content: [{ type: 'text', text: 'Here are the strongest ideas from eureka-db.' }] },
];

test('finished work folds into a summary, and MCP calls read as server + action', async ({ page }, testInfo) => {
  await installDesktopMock(page, { history: MCP_RUN });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('Here are the strongest ideas from eureka-db.')).toBeVisible();
  const summary = page.getByRole('button', { name: /Worked for 41s/ });
  await expect(summary).toContainText('eureka-db ×3');
  await expect(summary).toContainText('1 MCP script');
  await expect(summary).toContainText('3 thoughts');
  await expect(summary).toHaveAttribute('aria-expanded', 'false');
  await page.screenshot({ path: testInfo.outputPath('steps-folded.png') });
  await summary.click();
  await expect(summary).toHaveAttribute('aria-expanded', 'true');
  await expect(page.getByText('Connected to', { exact: true })).toBeVisible();
  const search = page.locator('.tool', { hasText: 'Search business ideas' });
  await expect(search).toContainText('eureka-db');
  await expect(search).toContainText('min opportunity score 80');
  await expect(search).toContainText('21 results');
  await expect(page.locator('.tool', { hasText: 'Get saved ideas' })).toContainText('3 results');
  await expect(page.locator('.tool', { hasText: 'Ran MCP script' })).toContainText('const r = await tools');
  await page.screenshot({ path: testInfo.outputPath('steps-open.png') });
  await search.getByRole('button').first().click();
  await expect(search).toContainText('has_revenue_signal');
});

test('slash commands: Pi built-ins run in πDesk, Pi commands go to Pi, terminal-only ones explain', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  const box = page.getByRole('combobox', { name: 'Message' });
  await box.fill('/');
  const menu = page.getByRole('listbox', { name: 'Commands' });
  await expect(menu.getByRole('option', { name: /\/model/ })).toContainText('πDesk');
  await box.fill('/mc');
  await expect(menu.getByRole('option', { name: /\/mcp-auth/ })).toContainText('Extension');
  await expect(menu.getByRole('option', { name: /\/skill:mcp-scripting/ })).toContainText('Skill');
  const sent = () => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => ['send_prompt', 'compact_thread', 'get_usage'].includes(call.command)).map((call: { command: string; args: unknown }) => [call.command, call.args]));
  const send = async (text: string) => { await box.fill(text); await box.press('Escape'); await page.getByRole('button', { name: 'Send message' }).click(); };

  await send('/compact keep the API notes');
  await expect(page.getByText('Compacted: older context is now a summary.')).toBeVisible();
  await send('/session');
  await expect(page.getByText(/1,500 tokens \(1,200 in, 300 out\) · \$0\.0123 · context 1% full/)).toBeVisible();
  await send('/tree');
  await expect(page.getByText(/\/tree only exists in Pi's own terminal UI/)).toBeVisible();
  await send('/mcp-auth eureka-db');
  await expect.poll(sent).toEqual([
    ['compact_thread', { threadId: 'a', instructions: 'keep the API notes' }],
    ['get_usage', { threadId: 'a' }],
    ['send_prompt', { threadId: 'a', message: '/mcp-auth eureka-db', mode: 'prompt' }],
  ]);
  // A slash command never becomes the thread title.
  await expect(page.locator('.top-thread')).toHaveText('Alpha');
});

test('@ mentions pick files and folders from anywhere under the thread folder', async ({ page }, testInfo) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  const box = page.getByRole('combobox', { name: 'Message' });
  await box.click();
  await box.pressSequentially('Explain @compo', { delay: 10 });
  const files = page.getByRole('listbox', { name: 'Files' });
  await expect(files.getByRole('option').first()).toContainText('Composer.svelte');
  await expect(files.getByRole('option').first()).toContainText('src/lib/components/conversation/');
  await page.screenshot({ path: testInfo.outputPath('mentions.png') });
  await box.press('Enter');
  await expect(box).toHaveValue('Explain @src/lib/components/conversation/Composer.svelte ');
  await expect(files).toHaveCount(0);
  // Folders are mentionable too, and Escape closes the list without leaving the box.
  await box.pressSequentially('and @src/li', { delay: 10 });
  await expect(files.getByRole('option').first()).toContainText('lib/');
  await box.press('Enter');
  await expect(box).toHaveValue('Explain @src/lib/components/conversation/Composer.svelte and @src/lib/ ');
  await box.pressSequentially('@read', { delay: 10 });
  await expect(files).toBeVisible();
  await box.press('Escape');
  await expect(files).toHaveCount(0);
  await expect(box).toBeFocused();
  // An email address is not a mention.
  await box.fill('mail me@example');
  await expect(files).toHaveCount(0);
});

test('Tab on a folder opens it in the mention list, level by level', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  const box = page.getByRole('combobox', { name: 'Message' });
  await box.click();
  await box.pressSequentially('@src', { delay: 10 });
  const files = page.getByRole('listbox', { name: 'Files' });
  await expect(files.getByRole('option').first()).toContainText('src/');
  await box.press('Tab');
  await expect(box).toHaveValue('@src/');
  await expect(files).toContainText('Inside src/');
  await expect(files.getByRole('option')).toHaveText([/lib\//, /app\.css/, /App\.svelte/]);
  await box.press('Tab');
  await expect(box).toHaveValue('@src/lib/');
  await expect(files.getByRole('option')).toHaveText([/components\//, /api\.ts/]);
  await files.getByRole('option', { name: /components\// }).locator('.open-folder').dispatchEvent('mousedown');
  await expect(box).toHaveValue('@src/lib/components/');
  await expect(files.getByRole('option')).toHaveText([/conversation\//]);
  await box.press('Tab');
  await box.press('Enter');
  await expect(box).toHaveValue('@src/lib/components/conversation/Composer.svelte ');
});

test('Enter sends, Shift+Enter adds a line, and the ↵ switch is remembered', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  const box = page.getByRole('combobox', { name: 'Message' });
  const prompts = () => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'send_prompt').map((call: { args: { message: string } }) => call.args.message));
  await box.click();
  await box.pressSequentially('first line', { delay: 5 });
  await box.press('Shift+Enter');
  await box.pressSequentially('second line', { delay: 5 });
  await expect(box).toHaveValue('first line\nsecond line');
  await box.press('Enter');
  await expect.poll(prompts).toEqual(['first line\nsecond line']);
  await expect(box).toHaveValue('');
  // Switched to ⌘Enter, Enter adds a line; the choice survives a reload.
  await page.getByRole('button', { name: 'Send with Enter' }).click();
  await page.reload();
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await box.click();
  await box.pressSequentially('a', { delay: 5 });
  await box.press('Enter');
  await expect(box).toHaveValue('a\n');
  await expect(page.getByRole('button', { name: 'Send with Enter' })).toHaveAttribute('aria-pressed', 'false');
});

test('while a run is going, the Steer/Queue menu opens and sends the chosen mode', async ({ page }, testInfo) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.evaluate(() => (window as any).__mockDesktop.emit({ type: 'rpc', threadId: 'a', frame: { type: 'agent_start' } }));
  const modeButton = page.getByRole('button', { name: 'Send mode while running' });
  await expect(modeButton).toHaveText(/Steer/);
  await modeButton.click();
  const queue = page.getByRole('menuitemradio', { name: /Queue/ });
  await expect(queue).toBeInViewport();
  await page.screenshot({ path: testInfo.outputPath('steer-menu.png') });
  await queue.click();
  await expect(modeButton).toHaveText(/Queue/);
  const box = page.getByRole('combobox', { name: 'Message' });
  await box.fill('then write the tests');
  await page.getByRole('button', { name: 'Queue follow-up' }).click();
  await modeButton.click();
  await page.getByRole('menuitemradio', { name: /Steer/ }).click();
  await box.fill('use the existing helper instead');
  await page.getByRole('button', { name: 'Send steer message' }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'send_prompt').map((call: { args: unknown }) => call.args))).toEqual([
    { threadId: 'a', message: 'then write the tests', mode: 'follow_up' },
    { threadId: 'a', message: 'use the existing helper instead', mode: 'steer' },
  ]);
});

test('the top bar names the thread worktree: its branch, else a short name, and opens it in Finder', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await page.evaluate(() => { (window as any).__mockDesktop.setWorktree('a', 0); (window as any).__git.branch = null; });
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  const chip = page.locator('.worktree-chip');
  await expect(chip).toHaveText('worktree a');
  // Once the agent creates a branch there, the chip shows it.
  await page.evaluate(() => { (window as any).__git.branch = 'feat/dark-mode'; (window as any).__mockDesktop.emit({ type: 'git_changed', threadId: 'a' }); });
  await expect(chip).toHaveText('feat/dark-mode');
  await chip.click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'plugin:opener|reveal_item_in_dir').length)).toBe(1);
  // A thread without a worktree shows no chip.
  await page.locator('.thread-link[title="Beta"]').click();
  await expect(page.getByText('History b')).toBeVisible();
  await expect(chip).toHaveCount(0);
});

test('a working thread shows a live clock and says what it is doing', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  const emit = (frame: unknown) => page.evaluate(value => (window as any).__mockDesktop.emit({ type: 'rpc', threadId: 'a', frame: value }), frame);
  await emit({ type: 'agent_start' });
  await emit({ type: 'tool_execution_start', toolCallId: 't1', toolName: 'read', args: { path: 'src/app.css' } });
  await expect(page.getByText('Reading app.css…')).toBeVisible();
  await expect(page.locator('.status-pill .pill-clock')).toHaveText(/^0:0\d$/);
  await expect(page.locator('.thread-row', { hasText: 'Alpha' }).locator('.meta-working')).toHaveText(/^Working 0:0\d$/);
});

test('plan feedback is steered to the agent before the plan is answered', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.evaluate(frame => (window as any).__mockDesktop.emit({ type: 'rpc', threadId: 'a', frame }), PLAN_FRAME('plan-2'));
  const review = page.getByRole('dialog', { name: 'Plan review' });
  await review.getByRole('button', { name: 'Give feedback' }).click();
  await review.getByRole('textbox').fill('Skip the tests, just the toggle');
  await review.getByRole('button', { name: /Send feedback/ }).click();
  await expect(review).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'respond_ui' || call.command === 'send_prompt').map((call: { command: string; args: unknown }) => [call.command, call.args]));
  expect(calls).toEqual([
    ['send_prompt', { threadId: 'a', message: 'Skip the tests, just the toggle', mode: 'steer' }],
    ['respond_ui', { threadId: 'a', requestId: 'plan-2', response: { value: 'Revise with feedback' } }],
  ]);
});

test('Intern is locked to Plan and its plans open beside the panel, even when hidden', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await page.getByRole('button', {name:'Ask Pi Intern'}).click();
  const chat = page.getByRole('dialog', {name:'Pi Intern',exact:true});
  await expect(chat).toContainText('Plan mode · read-only until you approve a plan');
  await expect(chat.getByRole('radiogroup', { name: 'Agent mode' })).toHaveCount(0);
  await chat.getByRole('button', {name:'Hide Pi Intern'}).click();
  await expect(chat).toBeHidden();
  await page.evaluate(frame => (window as any).__mockDesktop.emit({ type: 'rpc', threadId: 'pidesk-intern', frame }), PLAN_FRAME('auto-1'));
  await expect(chat).toBeVisible();
  const review = page.getByRole('dialog', { name: 'Plan review' });
  await expect(review).toContainText('Pi Intern');
  await expect(review.getByRole('heading', { name: 'Add dark mode toggle' })).toBeVisible();
  await review.getByRole('button', { name: 'Decline' }).click();
  await expect(review).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'respond_ui').map((call: { args: unknown }) => call.args))).toEqual([{ threadId: 'pidesk-intern', requestId: 'auto-1', response: { value: 'Decline' } }]);
});

test('a pending send cannot rename the thread selected afterward', async ({ page }) => {
  await installDesktopMock(page, { titles: ['New thread', 'New thread'], sendDelay: 200 });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link').first().click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.getByRole('combobox', { name: 'Message' }).fill('Hello from A');
  await page.getByRole('button', { name: 'Send message' }).click();
  await page.locator('.thread-link').nth(1).click();
  await expect(page.getByText('History b')).toBeVisible();
  await page.waitForTimeout(250);
  await expect(page.locator('.top-thread')).toHaveText('New thread');
  await expect(page.locator('.thread-link[title="Hello from A"]')).toHaveCount(1);
});

test('Enter on Cancel never confirms file revert', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();
  await page.getByRole('button', { name: 'Toggle changes' }).click();
  await expect(page.getByText('1 file changed')).toBeVisible();
  await page.locator('.file-row').click();
  await page.getByTitle('Revert file to HEAD').click();
  const dialog = page.getByRole('alertdialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Cancel' })).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(dialog).toBeHidden();
  await expect(page.locator('.diff-path-text')).toHaveText('sample.txt');
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.some((call: { command: string }) => call.command === 'git_revert_file'))).toBe(false);
});

test('benign ResizeObserver warnings stay silent and project removal lives in the project menu', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  await page.evaluate(() => window.dispatchEvent(new ErrorEvent('error', { message: 'ResizeObserver loop completed with undelivered notifications.' })));
  await expect(page.getByRole('alert')).toHaveCount(0);

  await expect(page.getByRole('menuitem', { name: 'Remove from app' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Actions for desktop-e2e' }).click();
  await expect(page.getByRole('menuitem', { name: 'Remove from app' })).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menuitem', { name: 'Remove from app' })).toHaveCount(0);
});

test('mermaid fences render as sanitized diagrams with a code toggle', async ({ page }) => {
  await installDesktopMock(page, {
    assistantText: [
      'Architecture:',
      '',
      '```mermaid',
      'flowchart TD',
      '  Client[Client App] --> Gateway[API Gateway]',
      '  Gateway --> Catalog[Catalog Service]',
      '  click Client "javascript:document.title=\'XSS-M\'"',
      '```',
    ].join('\n'),
  });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  const diagram = page.getByRole('img', { name: 'Mermaid diagram' });
  await expect(diagram.locator('svg')).toBeVisible({ timeout: 15_000 });
  await expect(diagram).toContainText('API Gateway');
  const probe = await diagram.evaluate((node) => ({
    scripts: node.querySelectorAll('script').length,
    anchors: node.querySelectorAll('a').length,
    foreign: node.querySelectorAll('foreignObject').length,
    handlers: Array.from(node.querySelectorAll('*')).filter((el) => Array.from(el.attributes).some((a) => a.name.startsWith('on'))).length,
  }));
  expect(probe).toEqual({ scripts: 0, anchors: 0, foreign: 0, handlers: 0 });
  await expect(page).toHaveTitle('πDesk');

  await page.getByRole('button', { name: 'Code', exact: true }).click();
  await expect(page.getByText('flowchart TD')).toBeVisible();
  await expect(diagram).toHaveCount(0);
});

test('invalid mermaid falls back to code with a readable error', async ({ page }) => {
  await installDesktopMock(page, { assistantText: '```mermaid\nflowchart TD\n  A -->\n```' });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText(/Couldn’t render diagram/)).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText('flowchart TD')).toBeVisible();
  expect(await page.evaluate(() => document.querySelectorAll('body > [id^="dpidesk-mermaid"], body > [id^="pidesk-mermaid"]').length)).toBe(0);
});

test('the open project collapses and re-expands from its row', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  const toggle = page.getByRole('button', { name: 'Open desktop-e2e' });
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('.thread-link[title="Alpha"]')).toBeVisible();
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await expect(page.locator('.thread-link[title="Alpha"]')).toHaveCount(0);
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-expanded', 'true');
  await expect(page.locator('.thread-link[title="Alpha"]')).toBeVisible();
});

test('runtime monitor shows per-project Pi memory and stops only idle threads', async ({ page }) => {
  await installDesktopMock(page);
  const mb = 1024 * 1024;
  await page.addInitScript((mb) => {
    (window as any).__runtimeStats = {
      appBytes: 80 * mb,
      threads: [
        { threadId: 'a', projectId: 'project-1', title: 'Alpha', pid: 101, memoryBytes: 210 * mb, processCount: 3, busy: false, idleSeconds: 30 },
        { threadId: 'b', projectId: 'project-1', title: 'Beta', pid: 102, memoryBytes: 150 * mb, processCount: 1, busy: false, idleSeconds: 600 },
        { threadId: 'c', projectId: 'project-1', title: 'Gamma', pid: 103, memoryBytes: 140 * mb, processCount: 1, busy: true, idleSeconds: 0 },
      ],
    };
  }, mb);
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();

  const pill = page.getByRole('button', { name: /Pi runtime: 3 running, 500 MB/ });
  await expect(pill).toBeVisible();
  await pill.click();
  const panel = page.getByRole('dialog', { name: 'Running Pi instances' });
  await expect(panel.getByText('desktop-e2e')).toBeVisible();
  await expect(panel.getByText('210 MB')).toBeVisible();
  await expect(panel.getByText('Working')).toBeVisible();
  await expect(panel.getByRole('button', { name: 'Stop Gamma' })).toBeDisabled();

  // Alpha is open in the main pane and Gamma is busy: bulk stop only takes Beta.
  await panel.getByRole('button', { name: 'Stop idle threads (1)' }).click();
  await expect(panel.getByText('Beta')).toHaveCount(0);
  const stopped = await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'stop_thread').map((call: { args: { threadId: string } }) => call.args.threadId));
  expect(stopped).toEqual(['b']);
  await expect(panel.getByRole('button', { name: 'Stop Alpha' })).toBeEnabled();
  await page.keyboard.press('Escape');
  await expect(panel).toHaveCount(0);
});

test('a new thread opens instantly while its Pi is still starting', async ({ page }) => {
  await installDesktopMock(page, { openDelay: { 'new-thread': 2_000 } });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();

  const started = Date.now();
  await page.locator('.new-thread').click();
  // The composer for the new thread is usable before open_thread resolves.
  await expect(page.locator('.top-thread')).toHaveText('New thread');
  await expect(page.getByRole('combobox', { name: 'Message' })).toBeEnabled();
  await expect(page.getByText('No messages yet')).toBeVisible();
  expect(Date.now() - started).toBeLessThan(1_000);
  await expect(page.getByRole('heading', { name: 'Opening thread' })).toHaveCount(0);
  // When Pi is ready, its snapshot is merged into the same view.
  await expect(page.getByText('History new-thread')).toBeVisible({ timeout: 5_000 });
});

test('double-click rename keeps focus even when the thread finishes opening afterwards', async ({ page }) => {
  await installDesktopMock(page, { openDelay: { a: 600 } });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').dblclick();
  const input = page.getByRole('textbox', { name: 'Thread title' });
  await expect(input).toBeFocused();
  // The conversation loads while the rename box is open.
  await expect(page.getByText('History a')).toBeVisible();
  await expect(input).toBeFocused();
  await input.fill('Renamed Alpha');
  await input.press('Enter');
  await expect(page.locator('.thread-link[title="Renamed Alpha"]')).toBeVisible();
});

test('hovering a thread prewarms its Pi, but passing over it does not', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await openProject(page);
  const prewarms = () => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'prewarm_thread').map((call: { args: { threadId: string } }) => call.args.threadId));

  // A quick pass over Alpha on the way to Beta must not start Alpha's Pi.
  // Move the mouse directly: hover() waits between steps, which on a slow
  // machine can itself exceed the 120 ms dwell.
  const center = async (title: string) => {
    const box = (await page.locator(`.thread-link[title="${title}"]`).boundingBox())!;
    return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  };
  const [alpha, beta] = [await center('Alpha'), await center('Beta')];
  await page.mouse.move(alpha.x, alpha.y);
  await page.mouse.move(beta.x, beta.y);
  await page.waitForTimeout(400);
  expect(await prewarms()).toEqual(['b']);

  // Hovering again within the cooldown does not spawn duplicates.
  await page.mouse.move(0, 0);
  await page.locator('.thread-link[title="Beta"]').hover();
  await page.waitForTimeout(400);
  expect(await prewarms()).toEqual(['b']);
});

test('model picker groups by provider, shows context, searches, and sets the default', async ({ page }, testInfo) => {
  await installDesktopMock(page, { models: true, openDelay: { 'new-thread': 2_000 } });
  await page.goto('/');
  await openProject(page);
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();

  const trigger = page.getByRole('button', { name: 'Select model' });
  await expect(trigger).toContainText('Kimi K2.6');
  await expect(trigger).toContainText('262K');

  await trigger.click();
  const panel = page.getByRole('dialog', { name: 'Choose a model' });
  // Providers sit in a rail; the current model's provider is first and open.
  const providers = panel.getByRole('tablist', { name: 'Providers' });
  await expect(providers.getByRole('tab')).toHaveText([/OpenCode Go\s*3/, /OpenAI Codex\s*1/]);
  await expect(providers.getByRole('tab', { name: /OpenCode Go/ })).toHaveAttribute('aria-selected', 'true');
  await expect(panel.getByRole('group', { name: 'OpenCode Go' })).toBeVisible();
  await expect(panel.getByRole('group', { name: 'OpenAI Codex' })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath('model-picker.png') });
  // The keyboard position starts on the current model.
  await expect(panel.getByRole('option', { name: /Kimi K2\.6/ })).toHaveAttribute('data-active', 'true');
  await expect(panel.getByRole('option', { name: /Kimi K2\.6/ })).toHaveAttribute('aria-selected', 'true');
  await expect(panel.getByRole('option', { name: /DeepSeek V4\.1 Flash/ })).toContainText('1M');
  await expect(panel.getByRole('option', { name: /DeepSeek V4\.1 Flash/ })).toContainText('Free');
  // Switch providers by click or with ←/→.
  await providers.getByRole('tab', { name: /OpenAI Codex/ }).click();
  await expect(panel.getByRole('group', { name: 'OpenAI Codex' })).toBeVisible();
  await expect(panel.getByRole('option')).toHaveCount(1);
  await panel.getByRole('combobox', { name: 'Search models' }).press('ArrowRight');
  await expect(panel.getByRole('group', { name: 'OpenCode Go' })).toBeVisible();
  await panel.getByRole('combobox', { name: 'Search models' }).focus();

  // Search across providers, then pick with the keyboard.
  await page.keyboard.type('codex spark');
  await expect(panel.getByRole('option')).toHaveCount(1);
  await page.keyboard.press('Enter');
  await expect(panel).toHaveCount(0);
  const setModel = await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'set_thread_model').map((call: { args: unknown }) => call.args));
  expect(setModel).toEqual([{ threadId: 'a', provider: 'openai-codex', modelId: 'gpt-5.3-codex-spark' }]);
  await expect(trigger).toContainText('GPT-5.3 Codex Spark');
  await expect(trigger).toContainText('128K');

  // Star a model as the default for new threads (now under the other provider).
  await trigger.click();
  await expect(panel.getByRole('group', { name: 'OpenAI Codex' })).toBeVisible();
  await panel.getByRole('tab', { name: /OpenCode Go/ }).click();
  await panel.getByRole('option', { name: /GLM-5\.1/ }).hover();
  await panel.getByRole('button', { name: 'Make GLM-5.1 the default for new threads' }).click();
  await expect(panel.getByRole('option', { name: /GLM-5\.1/ })).toContainText('Default');
  await page.keyboard.press('Escape');
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'set_default_model').map((call: { args: unknown }) => call.args))).toEqual([{ provider: 'opencode-go', modelId: 'glm-5.1' }]);

  // Settings shows and edits the same defaults.
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await settings.getByRole('button', { name: 'Models' }).click();
  await expect(settings.getByRole('button', { name: 'Default model for new threads' })).toContainText('GLM-5.1');
  await settings.getByRole('combobox', { name: 'Default effort for new threads' }).selectOption('high');
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.some((call: { command: string; args: { level?: string } }) => call.command === 'set_default_thinking_level' && call.args.level === 'high'))).toBe(true);
  await settings.getByRole('button', { name: 'Close settings' }).click();

  // A new thread starts on the default model immediately, before Pi is ready.
  await page.locator('.new-thread').click();
  await expect(page.locator('.top-thread')).toHaveText('New thread');
  await expect(page.getByRole('button', { name: 'Select model' })).toContainText('GLM-5.1');
});
