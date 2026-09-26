# πDesk

A local-first desktop app for [Pi](https://github.com/earendil-works/pi) coding-agent sessions.

πDesk provides project and thread management, streaming conversations, structured tool activity, extension prompts, and Git-backed diff review in a native macOS app. **Pi is the only supported coding agent.**

## Features

- Native Tauri 2 shell with Svelte 5 and TypeScript
- Private Pi installation with an opt-in installer and live terminal output
- Local SQLite metadata for projects and threads
- Pi JSONL RPC integration; no terminal scraping
- Streaming assistant responses and structured tool cards, including extension tools
- Rendered Markdown with syntax-highlighted code and Mermaid diagrams (Diagram/Code toggle)
- Extension input, selection, confirmation, and editor requests
- Model, effort, context, and token usage where supported by Pi
- Git-backed unified and split diffs with syntax highlighting
- File and hunk revert, word-level changes, and manual-edit refresh
- Worktree isolation for concurrent modifying Git threads
- Idle process suspension and crash recovery
- Runtime monitor: memory per running Pi, grouped by project, with one-click stop for idle threads
- Dark and light themes with keyboard navigation

## Requirements

- macOS with Xcode Command Line Tools
- [Bun](https://bun.sh/) 1.4.2 (pinned in `.bun-version`)
- Rust 1.98.1 (pinned in `rust-toolchain.toml`)
- Node.js 22.19+ and npm for the private Pi installer

## Private Pi setup

πDesk does **not** use Pi from your PATH or your existing `~/.pi` directory. On launch it checks only its own installation. If Pi is missing, the setup screen offers **Install Pi**. No installation happens until you click it.

The installer shows the exact local npm command, live stdout/stderr, and real check/install/verify stages. Errors stay visible and can be retried. npm installs `@earendil-works/pi-coding-agent` locally with scripts disabled—not with `-g`.

```text
~/.pidesk/
  runtime/      πDesk's Pi package and dependencies
  agent/        Private settings, auth, extensions, and sessions
  worktrees/    Isolated Git worktrees
```

After installation, copy the **private Pi sign-in command** from setup or Settings into Terminal, then use `/login`. A bare `pi` command opens your separate terminal installation, not πDesk's profile. No credentials or extensions are copied from it. There is no in-app provider OAuth flow or built-in subagent transcript browser.

## Development

```bash
bun install --frozen-lockfile
bun run tauri dev
```

The Vite frontend runs on `http://127.0.0.1:1420`; Tauri owns the native lifecycle and Rust backend.

## Validation

```bash
bun run check                          # Svelte/TypeScript, Rust fmt and check
bun run test                           # Vitest
bunx playwright install chromium       # browser setup
bun run test:e2e                        # mocked desktop journeys
bun run test:rust                      # Rust unit and transport tests
bun run build                          # frontend production build
bun run validate                       # full CI parity, including macOS bundle
```

Optional live RPC tests require an explicitly installed private Pi:

```bash
cd src-tauri
cargo test --test rpc_live -- --ignored
```

Live tests start real Pi processes; prompt tests may consume provider quota. They are not part of default validation.

## Production Build

```bash
bun run bundle:macos
```

The app bundle is written to:

```text
src-tauri/target/release/bundle/macos/πDesk.app
```

This command builds, ad-hoc signs, and verifies the bundle with `codesign --verify --strict`, preserving signature policy flags and entitlements. The result is for local use only: distribution requires a Developer ID signature and Apple notarization.

## Project Layout

```text
src/
  App.svelte                    Application shell and navigation
  lib/
    api.ts                      Typed Tauri command/event bridge
    session.svelte.ts           Pi event normalization and state
    components/
      conversation/             Transcript, composer, prompts, Markdown
      diff/                     Git status, file diff, revert actions
      tools/                    Structured tool renderers
      setup/                    Private installer and sign-in guidance
      runtime/                  Pi memory monitor and idle-thread controls
src-tauri/
  src/
    commands.rs                 Tauri command boundary
    dto.rs                      IPC data types
    git.rs                      Git status, diff, safe file operations
    harness.rs                  Managed Pi validation and opt-in installation
    rpc.rs                      Bounded JSONL transport and correlation
    sessions.rs                 Pi session discovery
    store.rs                    SQLite metadata and legacy compatibility
    threads.rs                  Process lifecycle and thread orchestration
  capabilities/                 Tauri permission allowlists
  tests/                        Transport and optional live Pi tests
```

## Local Data and Security

- Repository contents and app metadata stay local; there is no product telemetry. Pi's configured providers still receive requests through Pi.
- πDesk's Pi owns its private sessions, credentials, extensions, and provider connections under `~/.pidesk/agent`.
- Inherited Pi directory/package overrides and provider-key variables are not forwarded. Project `.pi` resources are skipped with `--no-approve`, and explicit private session directories override project session settings.
- This is installation/configuration separation, not an OS sandbox: Pi still works on selected project files with your user account's permissions.
- Git writes are restricted to changed paths and guarded by expected-content hashes.
- Removing a project deletes app metadata only, not files, Git history, worktrees, or Pi sessions.
- New isolated worktrees live under `~/.pidesk/worktrees/`; saved worktree paths remain usable.
- Fresh installs use `pidesk.sqlite3` in the `dev.pidesk.desktop` app-data directory.

### Existing installations

Projects are preserved, but πDesk starts with fresh private threads. Existing external-Pi and OMP thread rows remain stored but hidden and cannot be resumed with the managed runtime. Their original session files, CLI installations, credentials, and extensions are not modified or uninstalled. New threads are explicitly marked as belonging to πDesk's runtime.

When no new app database exists, πDesk can reuse the prior `dev.ompui.desktop/omp-desktop.sqlite3` metadata database in place to preserve projects, without copying live SQLite/WAL files. Session discovery only scans the private `~/.pidesk/agent/sessions` tree; external directory overrides are ignored.

The new app identity may reset window placement and webview preferences. The `ompui`/`omp-desktop` names above remain only so existing installations can be found; everything else is named πDesk.

## License

MIT
