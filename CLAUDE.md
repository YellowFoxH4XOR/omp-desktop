# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

OMP Desktop is a native macOS Tauri 2 app (Svelte 5 + TypeScript frontend, Rust backend) that drives local OMP and Pi coding-agent processes over JSON-RPC, and provides Git-backed diff review of the changes those agents make. Everything is local-first: SQLite metadata, no telemetry, no stored provider credentials (auth stays owned by OMP/Pi).

## Commands

```bash
bun install --frozen-lockfile   # install exact locked deps

bun run tauri dev               # run the app (Vite frontend on :1420 + Rust backend)

bun run check                   # svelte-check + `cargo fmt --check` + `cargo check --locked`
bun run check:frontend          # svelte-check only
bun run check:rust              # cd src-tauri && cargo fmt -- --check && cargo check --locked

bun run test                    # vitest (excludes tests/e2e)
bun run test:watch              # vitest watch mode
bunx playwright install chromium && bun run test:e2e   # Playwright desktop journeys
bun run test:rust               # cd src-tauri && cargo test --locked

bun run validate                # frozen install + check + test + e2e + rust test + build (full CI parity; ./scripts/validate.sh)
bun run build                   # frontend build only
bun run bundle:macos            # tauri build + ad-hoc codesign + verify (scripts/sign-macos-bundle.sh)
```

Run a single Rust test: `cd src-tauri && cargo test <name> --locked`.
Run a single Vitest file: `bun run test src/lib/session.test.ts`.
Optional live RPC smoke tests against real installed OMP/Pi executables (can start real processes and consume provider quota): `cd src-tauri && cargo test --test rpc_live -- --ignored`.

## Architecture

### Process model

The Rust backend spawns OMP/Pi CLI processes per thread and talks to them over a line-delimited JSON-RPC protocol on stdin/stdout (`src-tauri/src/rpc.rs`). Frames are chunked/reassembled with hard size caps (1 MiB per OMP frame, 8 MiB for Pi's monolithic history frame, 64 MiB reassembled) — the transport is adversarial-input-hardened because it's parsing output from an external process. All RPC events are pushed to the frontend as a single `desktop-event` Tauri event, validated frontend-side with a Zod discriminated union (`src/lib/api.ts`) before ever reaching app state, since backend frames are treated as untrusted input.

`src-tauri/src/threads.rs` (largest file, ~2.3k lines) is the process lifecycle core: spawning, readiness handshakes (OMP vs Pi have different readiness signals — see `READY_TIMEOUT_SECS` vs `PI_READY_TIMEOUT_SECS`), idle suspension after 15 minutes (`IDLE_SUSPEND`), crash recovery, subagent tracking, and UI-interaction request/response correlation (permissions, select/confirm/input/editor prompts). A `LiveThread` tracks per-process busy state (streaming, pending UI requests, active subagents) so idle suspension never kills a process mid-work. Thread-scoped operations (prompt/stop/restart) are serialized per-thread via a global lock map (`shared_locks_map`) keyed by thread id, reaped once uncontended.

`src-tauri/src/watcher.rs` watches each active thread's working directory (filesystem + git metadata) on one dedicated OS thread and emits `git_changed` events — no polling.

`src-tauri/src/harness.rs` handles OMP/Pi discovery, executable-path overrides, and a fixed, allowlisted install-command table (never arbitrary argv) for the guided installer.

`src-tauri/src/sessions.rs` handles session discovery/replay and message-history pagination (bounded: `MAX_HISTORY_PAGES`/`MAX_HISTORY_MESSAGES`/`MAX_HISTORY_BYTES`) for resuming existing threads.

`src-tauri/src/git.rs` (largest non-lifecycle file, ~1.9k lines) implements Git status/diff and file write/revert. Write operations are restricted to paths Git currently reports as changed, and writes/reverts are guarded by expected-content-hash checks to avoid clobbering concurrent external edits.

`src-tauri/src/store.rs` is the SQLite layer for local metadata (projects, threads, settings) — not conversation content, which lives in OMP/Pi's own session files on disk.

Everything routes through `src-tauri/src/commands.rs`, the `#[tauri::command]` IPC boundary registered in `lib.rs`'s `invoke_handler!`; `src-tauri/src/dto.rs` defines the (de)serialized types shared with the frontend. `AppError`/`AppResult` (`error.rs`) is the single error type across the backend — `Display` text is what the UI shows directly, so it must never leak secrets, tokens, or raw protocol dumps.

`state.rs`'s `AppState` (store, harness registry, watcher, thread manager) is constructed once in `lib.rs`'s `setup` hook and managed by Tauri; on startup, threads from a previous run are marked disconnected before the UI loads since their OS processes are gone.

### Frontend

`src/lib/api.ts` is the sole typed bridge to Rust (`invoke` wrappers) and the single point where backend events are schema-validated before entering app state.

`src/lib/session.svelte.ts` (`SessionModel`) is the reactive core of one open thread: it flattens `get_messages` history into `ConversationItem` rows and reduces live RPC frames — from *both* OMP and Pi, which have different event shapes (OMP strips `assistantMessageEvent.partial`; Pi is delta-only) — into one normalized `SessionView`. UI components never see raw harness frames, only this normalized shape. This is the file to read first when changing how conversation/tool-call state is derived or displayed.

`src/App.svelte` is the application shell: project/thread list, session caching (`MAX_CACHED_SESSIONS`), and event wiring between `api.ts` and mounted `SessionModel`s.

`src/lib/components/` is organized by concern: `conversation/` (transcript, composer, markdown rendering, request cards), `tools/` (one renderer per structured tool-call type — command, edit, read, search, task/subagent, todo, web, generic fallback), `agents/` (subagent hierarchy + transcript inspection), `diff/` (Git status list, split/unified diff editor via CodeMirror merge, revert actions).

### Cross-cutting notes

- Two harnesses (OMP, Pi) are first-class throughout — protocol framing, readiness, event shapes, and install commands all branch on `HarnessKind`. When touching RPC/session/thread code, check both paths.
- Size/count limits are pervasive by design (frame bytes, history pages/messages/bytes, cached sessions, pending frames, stderr tail, live thread count) — this is a long-running desktop app talking to external processes, so unbounded growth is treated as a bug class. Preserve existing caps and add new ones for any new unbounded buffer.
- Local worktree isolation for concurrent modifying threads lives under `~/.omp-desktop/worktrees/`; project removal only deletes app metadata, never repo files, git history, worktrees, or harness session data.
