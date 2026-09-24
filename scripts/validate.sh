#!/usr/bin/env bash
set -euo pipefail

bun install --frozen-lockfile
bun run check
bun run test
bun run test:rust
bun run build
