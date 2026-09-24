import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests/e2e',
  timeout: 30_000,
  expect: { timeout: 5_000 },
  retries: 0,
  workers: 1,
  use: {
    baseURL: 'http://127.0.0.1:1420',
    ...devices['Desktop Chrome'],
  },
  webServer: {
    command: 'bunx vite dev --host 127.0.0.1',
    url: 'http://127.0.0.1:1420',
    reuseExistingServer: process.env.GITHUB_ACTIONS !== 'true',
    timeout: 120_000,
  },
});
