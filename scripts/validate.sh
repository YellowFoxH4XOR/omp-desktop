#!/usr/bin/env bash
set -euo pipefail

bun install --frozen-lockfile
bun run check
bun run test
bunx playwright install chromium
bun run test:e2e
bun run test:rust
bun run build

if [[ "$(uname -s)" == "Darwin" ]]; then
  bun run bundle:macos
else
  echo "Skipping signed macOS bundle validation on $(uname -s); run on macOS for full CI parity." >&2
fi
