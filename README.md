# OMP Desktop

A local-first visual control center for [OMP](https://github.com/oh-my-pi/pi-coding-agent) and Pi coding-agent sessions.

OMP Desktop provides projects and thread management, streaming conversations, structured tool activity, permissions, subagent transcripts, and Git-backed diff review in a native macOS desktop app.

## Features

- Native Tauri 2 shell with Svelte 5 and TypeScript
- Local SQLite metadata for projects and threads
- Structured OMP and Pi RPC adapters; no terminal scraping
- Streaming assistant responses and normalized tool cards
- Permission, input, selection, and plan-request UI
- Live nested subagent hierarchy and transcript inspection
- Model, effort, context, and token usage controls
- Git-backed unified and split diffs with syntax highlighting
- File and hunk revert, word-level changes, and manual-edit refresh
- Worktree isolation for concurrent modifying Git threads
- Idle process suspension and crash recovery
- Dark and light themes with keyboard navigation

## Requirements

- macOS with Xcode Command Line Tools
- [Bun](https://bun.sh/)
- Rust stable toolchain
- OMP or Pi installed, or use the guided installer from first launch

## Development

Install frontend dependencies:

```bash
bun install
```

Start the Tauri development app:

```bash
bun run tauri dev
```

The Vite frontend runs on `http://127.0.0.1:1420`; the Tauri window manages the native lifecycle and Rust backend.

## Validation

Run frontend type and Svelte checks:

```bash
bun run check
```

Run frontend reducer tests:

```bash
bun run test
```

Run Rust formatting, checks, and unit tests:

```bash
cd src-tauri
cargo fmt -- --check
cargo check
cargo test --lib
```

Run optional live RPC smoke tests against installed OMP and Pi executables:

```bash
cd src-tauri
cargo test --test rpc_live -- --ignored
```

These live tests may start real harness processes. Some tests can make a model request and consume the configured provider account.

## Production Build

Build the frontend only:

```bash
bun run build
```

Build the macOS application bundle:

```bash
bun run tauri build --bundles app
```

The local application bundle is written to:

```text
src-tauri/target/release/bundle/macos/OMP Desktop.app
```

The local build is ad-hoc signed. Distributing it to other Macs requires a Developer ID signature and Apple notarization.

## Project Layout

```text
src/
  App.svelte                    Application shell and navigation
  lib/
    api.ts                      Typed Tauri command/event bridge
    session.svelte.ts           RPC event normalization and state
    components/
      agents/                   Agent hierarchy and transcripts
      conversation/             Transcript, composer, requests, Markdown
      diff/                     Git status, file diff, revert actions
      tools/                    Structured tool renderers

src-tauri/
  src/
    commands.rs                 Tauri command boundary
    dto.rs                      IPC data types
    git.rs                      Git status, diff, and safe file operations
    harness.rs                  Harness discovery and guided installation
    rpc.rs                      JSONL transport, chunking, and correlation
    sessions.rs                 OMP/Pi session discovery and replay
    store.rs                    SQLite metadata
    threads.rs                  Process lifecycle and thread orchestration
  capabilities/                 Tauri permission allowlists
  tests/                        Live RPC smoke tests
```

## Local Data and Security

- Repository contents stay on the local machine.
- The app does not store provider credentials; authentication remains owned by OMP or Pi.
- Git write operations are restricted to paths currently reported as changed by Git.
- Project removal deletes app metadata only, not repository files, Git history, worktrees, or harness sessions.
- App-owned isolated worktrees are stored under `~/.omp-desktop/worktrees/`.

## License

MIT
