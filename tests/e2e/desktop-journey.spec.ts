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
}

const MODEL_CATALOGUE = [
  { provider: 'opencode-go', id: 'kimi-k2.6', name: 'Kimi K2.6', contextWindow: 262144, reasoning: true, images: true, cost: { input: 0.95, output: 4 } },
  { provider: 'opencode-go', id: 'glm-5.1', name: 'GLM-5.1', contextWindow: 200000, reasoning: true, cost: { input: 1, output: 3.2 } },
  { provider: 'opencode-go', id: 'deepseek-v4.1-flash', name: 'DeepSeek V4.1 Flash', contextWindow: 1048576, cost: { input: 0, output: 0 } },
  { provider: 'openai-codex', id: 'gpt-5.3-codex-spark', name: 'GPT-5.3 Codex Spark', contextWindow: 128000, reasoning: true, cost: { input: 1.75, output: 14 } },
];

async function installDesktopMock(page: Page, scenario: Scenario = {}) {
  const [firstTitle, secondTitle] = scenario.titles ?? ['Alpha', 'Beta'];
  await page.addInitScript(({ project, threads, openDelay, sendDelay, rememberedThreadId, assistantText, piInstalled, systemPiOnly, manualInstall, missingInstallPlan, deferEvents, catalogue }) => {
    let defaults: { provider?: string; modelId?: string; thinkingLevel?: string } = catalogue.length ? { provider: 'opencode-go', modelId: 'kimi-k2.6' } : {};
    localStorage.setItem('lastProject', project.id);
    if (rememberedThreadId) localStorage.setItem('lastThread', rememberedThreadId);
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    let nextCallback = 0;
    let nextListener = 0;
    let installed = piInstalled;
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
        messages: [{ role: 'assistant', content: [{ type: 'text', text: assistantText ?? `History ${id}` }], timestamp: 1 }],
        state: { sessionId: row.sessionId, sessionFile: row.sessionFile, isStreaming: false,
          ...(catalogue.length ? { model: catalogue.find(model => model.provider === defaults.provider && model.id === defaults.modelId) ?? catalogue[0], thinkingLevel: defaults.thinkingLevel ?? 'medium' } : {}) },
        models: clone(catalogue), levels: catalogue.length ? ['off', 'medium', 'high'] : [], agents: [],
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
        if (command === 'create_thread') {
          // A real new thread has no Pi session until Pi starts.
          const row = { ...threads[0], id: 'new-thread', title: 'New thread', harness: String(args.harness), sessionId: '', sessionFile: '' };
          threads.push(row);
          return clone(row);
        }
        if (command === 'list_projects') return [clone(project)];
        if (command === 'list_threads') return clone(threads);
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
          return { isRepo: true, branch: 'main', files: clone(changedFiles),
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
  });
}

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
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
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
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
  await expect(page.getByRole('combobox', { name: 'Harness for new threads' })).toHaveCount(0);
  await page.locator('.new-thread').click();
  await expect(page.getByText('History new-thread')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: any) => call.command === 'create_thread'))).toEqual([{ command: 'create_thread', args: { projectId: 'project-1', harness: 'pi', isolated: false } }]);
  await expect(page.getByRole('button', { name: 'Toggle agents' })).toHaveCount(0);
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Private Pi installation' })).toBeVisible();
  await expect(page.getByText('/Users/test/.pidesk/runtime/node_modules/.bin/pi', { exact: true })).toBeVisible();
  await expect(page.getByText('OMP', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Private Pi sign-in command' })).toBeVisible();
});

test('Settings shows full private paths and opens and closes the Pi terminal', async ({ page }) => {
  await installDesktopMock(page);
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.goto('/');
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
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
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'open_thread'))).toHaveLength(0);

  await page.locator('.thread-link[title="Alpha"]').click();
  await page.locator('.thread-link[title="Beta"]').click();
  await expect(page.locator('.top-thread')).toHaveText('Beta');
  await expect(page.getByText('History b')).toBeVisible();
  await page.waitForTimeout(250);
  await expect(page.locator('.top-thread')).toHaveText('Beta');
  await expect(page.getByText('History a')).toHaveCount(0);
});

test('remembered thread reopens on launch', async ({ page }) => {
  await installDesktopMock(page, { rememberedThreadId: 'a' });
  await page.goto('/');
  await expect(page.locator('.thread-row.active .thread-title')).toHaveText('Alpha');
  await expect(page.getByText('History a')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'open_thread').map((call: { args: { threadId: string } }) => call.args.threadId))).toEqual(['a']);
});

test('thread popover closes after actions and rename saves or cancels in place', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
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
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
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
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
  await page.locator('.thread-link[title="Alpha"]').click();
  await page.evaluate(() => (window as any).__mockDesktop.emit({
    type: 'rpc', threadId: 'a',
    frame: { type: 'extension_ui_request', id: 'approval-1', method: 'confirm',
      title: 'Approve change', message: 'May the agent edit this file?' },
  }));
  await expect(page.getByRole('group', { name: 'Request: Approve change' })).toBeVisible();
  await expect(page.getByText('May the agent edit this file?')).toBeVisible();
});

test('a pending send cannot rename the thread selected afterward', async ({ page }) => {
  await installDesktopMock(page, { titles: ['New thread', 'New thread'], sendDelay: 200 });
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
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
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
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
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
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
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText(/Couldn’t render diagram/)).toBeVisible({ timeout: 15_000 });
  await expect(page.getByText('flowchart TD')).toBeVisible();
  expect(await page.evaluate(() => document.querySelectorAll('body > [id^="dpidesk-mermaid"], body > [id^="pidesk-mermaid"]').length)).toBe(0);
});

test('the open project collapses and re-expands from its row', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
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

test('hovering a thread prewarms its Pi, but passing over it does not', async ({ page }) => {
  await installDesktopMock(page);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'What shall we work on?' })).toBeVisible();
  const prewarms = () => page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'prewarm_thread').map((call: { args: { threadId: string } }) => call.args.threadId));

  // A quick pass over Alpha on the way to Beta must not start Alpha's Pi.
  await page.locator('.thread-link[title="Alpha"]').hover();
  await page.locator('.thread-link[title="Beta"]').hover();
  await page.waitForTimeout(400);
  expect(await prewarms()).toEqual(['b']);

  // Hovering again within the cooldown does not spawn duplicates.
  await page.mouse.move(0, 0);
  await page.locator('.thread-link[title="Beta"]').hover();
  await page.waitForTimeout(400);
  expect(await prewarms()).toEqual(['b']);
});

test('model picker groups by provider, shows context, searches, and sets the default', async ({ page }) => {
  await installDesktopMock(page, { models: true, openDelay: { 'new-thread': 2_000 } });
  await page.goto('/');
  await page.locator('.thread-link[title="Alpha"]').click();
  await expect(page.getByText('History a')).toBeVisible();

  const trigger = page.getByRole('button', { name: 'Select model' });
  await expect(trigger).toContainText('Kimi K2.6');
  await expect(trigger).toContainText('262K');

  await trigger.click();
  const panel = page.getByRole('dialog', { name: 'Choose a model' });
  await expect(panel.getByRole('group', { name: 'OpenCode Go' })).toBeVisible();
  await expect(panel.getByRole('group', { name: 'OpenAI Codex' })).toBeVisible();
  // The current provider is listed first, and the current model is marked.
  await expect(panel.locator('.group-head').first()).toContainText('OpenCode Go');
  await expect(panel.getByRole('option', { name: /Kimi K2\.6/ })).toHaveAttribute('aria-selected', 'true');
  await expect(panel.getByRole('option', { name: /DeepSeek V4\.1 Flash/ })).toContainText('1M');
  await expect(panel.getByRole('option', { name: /DeepSeek V4\.1 Flash/ })).toContainText('Free');

  // Search across providers, then pick with the keyboard.
  await page.keyboard.type('codex spark');
  await expect(panel.getByRole('option')).toHaveCount(1);
  await page.keyboard.press('Enter');
  await expect(panel).toHaveCount(0);
  const setModel = await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'set_thread_model').map((call: { args: unknown }) => call.args));
  expect(setModel).toEqual([{ threadId: 'a', provider: 'openai-codex', modelId: 'gpt-5.3-codex-spark' }]);
  await expect(trigger).toContainText('GPT-5.3 Codex Spark');
  await expect(trigger).toContainText('128K');

  // Star a model as the default for new threads.
  await trigger.click();
  await panel.getByRole('option', { name: /GLM-5\.1/ }).hover();
  await panel.getByRole('button', { name: 'Make GLM-5.1 the default for new threads' }).click();
  await expect(panel.getByRole('option', { name: /GLM-5\.1/ })).toContainText('Default');
  await page.keyboard.press('Escape');
  expect(await page.evaluate(() => (window as any).__mockDesktop.calls.filter((call: { command: string }) => call.command === 'set_default_model').map((call: { args: unknown }) => call.args))).toEqual([{ provider: 'opencode-go', modelId: 'glm-5.1' }]);

  // Settings shows and edits the same defaults.
  await page.getByRole('button', { name: 'Settings', exact: true }).click();
  const settings = page.getByRole('dialog', { name: 'Settings' });
  await expect(settings.getByRole('button', { name: 'Default model for new threads' })).toContainText('GLM-5.1');
  await settings.getByRole('combobox', { name: 'Default effort for new threads' }).selectOption('high');
  await expect.poll(() => page.evaluate(() => (window as any).__mockDesktop.calls.some((call: { command: string; args: { level?: string } }) => call.command === 'set_default_thinking_level' && call.args.level === 'high'))).toBe(true);
  await settings.getByRole('button', { name: 'Close settings' }).click();

  // A new thread starts on the default model immediately, before Pi is ready.
  await page.locator('.new-thread').click();
  await expect(page.locator('.top-thread')).toHaveText('New thread');
  await expect(page.getByRole('button', { name: 'Select model' })).toContainText('GLM-5.1');
});
