import { afterEach, describe, expect, it } from 'vitest';
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';

const tempDirs: string[] = [];

afterEach(async () => {
  await Promise.all(tempDirs.splice(0).map((dir) => rm(dir, { recursive: true, force: true })));
});

async function runValidation(platform: 'Darwin' | 'Linux') {
  const bin = await mkdtemp(join(tmpdir(), 'omp-validate-'));
  tempDirs.push(bin);
  const log = join(bin, 'commands.log');

  await writeFile(
    join(bin, 'uname'),
    `#!/usr/bin/env bash\nprintf '%s\\n' '${platform}'\n`,
    { mode: 0o755 },
  );
  await writeFile(
    join(bin, 'bun'),
    `#!/usr/bin/env bash\nprintf '%s\\n' "$*" >> "${log}"\n`,
    { mode: 0o755 },
  );
  await writeFile(
    join(bin, 'bunx'),
    `#!/usr/bin/env bash\nprintf 'bunx %s\\n' "$*" >> "${log}"\n`,
    { mode: 0o755 },
  );

  const child = spawn('bash', ['scripts/validate.sh'], {
    cwd: process.cwd(),
    env: { ...process.env, PATH: `${bin}:${process.env.PATH ?? ''}` },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr += chunk;
  });
  const exitCode = await new Promise<number | null>((resolve, reject) => {
    child.once('error', reject);
    child.once('close', resolve);
  });

  expect(exitCode).toBe(0);
  return { commands: (await readFile(log, 'utf8')).trim().split('\n'), stderr };
}

describe.skipIf(process.platform === 'win32')('validate.sh', () => {
  it('matches the full macOS CI sequence including the signed bundle', async () => {
    const { commands, stderr } = await runValidation('Darwin');

    expect(commands).toEqual([
      'install --frozen-lockfile',
      'run check',
      'run test',
      'bunx playwright install chromium',
      'run test:e2e',
      'run test:rust',
      'run build',
      'run bundle:macos',
    ]);
    expect(stderr).not.toContain('Skipping signed macOS bundle');
  });

  it('runs every portable check elsewhere and reports the omitted bundle', async () => {
    const { commands, stderr } = await runValidation('Linux');

    expect(commands).toEqual([
      'install --frozen-lockfile',
      'run check',
      'run test',
      'bunx playwright install chromium',
      'run test:e2e',
      'run test:rust',
      'run build',
    ]);
    expect(stderr).toContain('Skipping signed macOS bundle validation on Linux');
  });
});
