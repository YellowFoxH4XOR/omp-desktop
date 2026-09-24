import { expect, test, type Page } from '@playwright/test';

const project = {
  id: 'project-1',
  path: '/tmp/omp-desktop-e2e',
  displayName: 'desktop-e2e',
  preferredHarness: 'omp',
  isGit: true,
  createdAt: '2026-01-01T00:00:00Z',
  lastOpenedAt: '2026-01-01T00:00:00Z',
};

function thread(id: string, title: string, viewedAt: string) {
  return {
    id,
    projectId: project.id,
    harness: 'omp',
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
  titles?: [string, string];
  openDelay?: Record<string, number>;
  sendDelay?: number;
}

async function installDesktopMock(page: Page, scenario: Scenario = {}) {
  const [firstTitle, secondTitle] = scenario.titles ?? ['Alpha', 'Beta'];
  await page.addInitScript(({ project, threads, openDelay, sendDelay }) => {
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<number, { event: string; handler: number }>();
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    let nextCallback = 0;
    let nextListener = 0;
    let changedFiles = [{ path: 'sample.txt', status: 'M', additions: 1, deletions: 1, binary: false }];
    const clone = <T>(value: T): T => structuredClone(value);
    const snapshot = (id: string) => {
      const row = threads.find(candidate => candidate.id === id);
      if (!row) throw new Error(`Unknown thread ${id}`);
      return {
        thread: clone(row),
        messages: [{ role: 'assistant', content: [{ type: 'text', text: `History ${id}` }], timestamp: 1 }],
        state: { sessionId: row.sessionId, sessionFile: row.sessionFile, isStreaming: false },
        models: [], levels: [], agents: [],
        capabilities: {
          agents: true, nestedAgents: true, agentSteering: false, agentKill: false,
          agentRevive: false, planMode: false, permissions: true, modelSwitching: false,
          effortLevels: false, contextUsage: false, tokenUsage: false, worktrees: true,
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
          return id;
        }
        if (command === 'plugin:event|unlisten') {
          listeners.delete(Number(args.eventId));
          return;
        }
        if (command === 'detect_harnesses') return [{ kind: 'omp', path: '/usr/local/bin/omp', version: '18.2.11', source: 'path' }];
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
        if (command === 'stop_thread' || command === 'respond_ui' || command === 'abort_thread') return;
        throw new Error(`Unexpected Tauri command: ${command}`);
      },
    };
    host.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    host.__mockDesktop = {
      calls,
      emit(payload: unknown) {
        for (const [id, listener] of listeners) {
          if (listener.event === 'desktop-event') callbacks.get(listener.handler)?.({ event: listener.event, id, payload });
        }
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
  });
}

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
