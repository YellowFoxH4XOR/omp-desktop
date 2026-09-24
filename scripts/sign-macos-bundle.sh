#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS bundle signing is only supported on macOS." >&2
  exit 1
fi

app_path="${1:-src-tauri/target/release/bundle/macos/OMP Desktop.app}"

if [[ ! -d "$app_path" ]]; then
  echo "Application bundle not found: $app_path" >&2
  echo "Run 'bun run bundle:macos' first." >&2
  exit 1
fi

codesign --force --deep --sign - --timestamp=none "$app_path"
codesign --verify --deep --strict --verbose=2 "$app_path"
codesign --display --verbose=4 "$app_path"
