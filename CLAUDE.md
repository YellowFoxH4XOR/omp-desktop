# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

πDesk is a native macOS Tauri 2 app (Svelte 5 + TypeScript frontend, Rust backend) that drives local Pi coding-agent processes over JSONL RPC and provides Git-backed diff review. Pi is the only supported agent, and πDesk uses only its private installation under `~/.pidesk/runtime`. Everything is local-first: SQLite metadata, no telemetry, and credentials owned by the private Pi profile under `~/.pidesk/agent`. The display name is `πDesk`; package/crate names use ASCII `pidesk`.

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
Optional live RPC smoke tests against the explicitly installed private Pi executable (can start real processes and consume provider quota): `cd src-tauri && cargo test --test rpc_live -- --ignored`.

## Architecture

### Process model

The Rust backend spawns Pi CLI processes per thread and talks to them over a line-delimited RPC protocol on stdin/stdout (`src-tauri/src/rpc.rs`). Frames have an 8 MiB hard cap, including Pi's monolithic history response; there is no chunk-reassembly protocol. The transport is adversarial-input-hardened because it parses output from an external process. All RPC events are pushed to the frontend as a single `desktop-event` Tauri event, validated frontend-side with a Zod discriminated union (`src/lib/api.ts`) before ever reaching app state, since backend frames are treated as untrusted input.

`src-tauri/src/threads.rs` is the process lifecycle core: spawning, readiness via `get_state`, idle suspension after 15 minutes (`IDLE_SUSPEND`), crash recovery, and extension UI request/response correlation (select/confirm/input/editor prompts). A `LiveThread` tracks busy state so idle suspension never kills a process mid-work. Pi can compact or drain queued work after `agent_end`; only `agent_settled` ends the run. Resume uses `--session <session-file>` only for registered managed threads whose journal exists inside private session storage; Pi writes the journal only after the first turn, so an unwritten one resumes with `--session-id <id>` (validated charset) and `adopt_state` accepts the new path only while the old file is absent. Every spawn also passes `--append-system-prompt` with `RENDERING_GUIDE`, telling the model πDesk renders Mermaid. Every spawn forces a private `--session-dir` and `--no-approve`, clears inherited Pi/provider configuration, and retains project cwd. Thread-scoped operations (prompt/stop/restart) are serialized per-thread via a global lock map (`shared_locks_map`) keyed by thread id, reaped once uncontended.

`src-tauri/src/watcher.rs` watches each active thread's working directory (filesystem + git metadata) on one dedicated OS thread and emits `git_changed` events — no polling.

`src-tauri/src/harness.rs` validates only πDesk's private Pi and runs the fixed allowlisted local npm install command (never arbitrary argv, no `-g`). There is no system-Pi discovery or executable override. Detection must not install or create a private runtime. Installation begins only from an explicit UI action and streams bounded/redacted output plus preparing/installing/verifying events. Pi authentication uses the isolated terminal command shown in setup/Settings, then `/login`; bare system `pi` must never be recommended for πDesk auth.

`src-tauri/src/extensions.rs` manages Pi packages for the private Pi only. Discovery parses pi.dev/packages cards (bounded; malformed cards skipped); installed state is the private `settings.json` `packages` plus `agent/npm/node_modules/*/package.json`; update checks use the npm registry. Network access is `/usr/bin/curl` with fixed arguments, an HTTPS-only protocol, and size/time caps. Install/update/remove run the private `pi` with exactly one source that `normalize_source` accepted (npm name or `git:host/owner/repo`; never a flag, local path, or shell text), serialized, outside any project. Settings is a full page (General, Models, Pi runtime, Extensions); pending package updates badge the Settings gear.

`src-tauri/src/mcp.rs` backs Settings → MCP servers. πDesk owns only `<agent dir>/mcp.json` (the adapter's Pi-global file); edits validate transport fields and pass other adapter fields through; raw saves keep comments. Shared MCP files in the real home (`~/.config/mcp/mcp.json`, `~/.agents/…`) are never loaded (private home) — they are listed without values and copied per server only on an explicit Copy, with the backend re-reading the file.

`src-tauri/src/sessions.rs` discovers only private Pi sessions under `~/.pidesk/agent/sessions`, including late `session_info` names. Metadata scans and history count/bytes are bounded. Inherited session/config directory overrides are ignored. Never import external Pi sessions, even if their headers match.

`src-tauri/src/git.rs` implements Git status/diff and file write/revert. Write operations are restricted to paths Git currently reports as changed, and writes/reverts are guarded by expected-content-hash checks to avoid clobbering concurrent external edits.

`src-tauri/src/store.rs` is the SQLite layer for local metadata (projects, threads, settings) — not conversation content, which lives in Pi's own session files on disk.

Everything routes through `src-tauri/src/commands.rs`, the `#[tauri::command]` IPC boundary registered in `lib.rs`'s `invoke_handler!`; `src-tauri/src/dto.rs` defines the (de)serialized types shared with the frontend. `AppError`/`AppResult` (`error.rs`) is the single error type across the backend — `Display` text is what the UI shows directly, so it must never leak secrets, tokens, or raw protocol dumps.

`state.rs`'s `AppState` (store, harness registry, watcher, thread manager) is constructed once in `lib.rs`'s `setup` hook and managed by Tauri; on startup, threads from a previous run are marked disconnected before the UI loads since their OS processes are gone.

### Frontend

`src/lib/api.ts` is the sole typed bridge to Rust (`invoke` wrappers) and the single point where backend events are schema-validated before entering app state.

`src/lib/session.svelte.ts` (`SessionModel`) is the reactive core of one open thread: it flattens `get_messages` history into `ConversationItem` rows and reduces Pi live RPC deltas into one normalized `SessionView`, reconciling final messages without duplicate text. UI components never see raw harness frames, only this normalized shape. This is the file to read first when changing how conversation/tool-call state is derived or displayed.

`src/App.svelte` is the application shell: project/thread list, session caching (`MAX_CACHED_SESSIONS`), and event wiring between `api.ts` and mounted `SessionModel`s. Launch always lands on the welcome screen (`components/welcome/`: projects plus `list_recent_threads`, which never touches project order) and starts no harness; the sidebar Home button returns there.

`src/lib/components/setup/` owns the opt-in private installer, terminal-style log, real stage progress, error/retry/success UI, and separate sign-in guidance. Installer events must be subscribed before enabling Install; never invent a fallback global command or simulated percentage.

Sidebar rows show live status (working clock from `activeSince`, *Needs approval* when a plan request is pending), Plan mode, and Git branch/`+/−` only for isolated worktrees (`rowStats`, refreshed on `git_changed`); a shared checkout's diff is never attributed to one thread. A pending plan also turns the composer into an approval strip (`Composer` `approval`, facts from `planFacts` in `src/lib/plan.ts`), and `WorkingLine` narrates the running step under the transcript.

`src/lib/components/` is organized by concern: `conversation/` (transcript, composer, markdown rendering, request cards; `mermaid.ts` lazy-loads Mermaid in strict mode and re-sanitizes its SVG—model-authored diagrams are untrusted), `runtime/` (`RuntimeMonitor`, polling `get_runtime_stats` for per-thread physical footprint of each Pi process group via `util::process_group_footprint`; bulk stop never touches busy threads or the open thread), `tools/` (one renderer per structured tool-call type — command, edit, read, search, todo, web, and generic fallback for extension tools), `diff/` (Git status list, split/unified diff editor via CodeMirror merge, revert actions).

### Pi Intern

`src-tauri/src/intern.rs` brokers the bundled `intern-extension.mjs` for the hidden coordinator (`intern_threads`). The coordinator runs with Pi's built-in tools and installed extensions plus the bridge's `pidesk` tool; confirming edits/commands in chat is instruction-only (`SYSTEM`, appended), while `pidesk` app actions are typed, immutable host-approved plans. Threads it starts or messages are ordinary full-Pi project threads (`create_intern_thread`/`send_prompt`); startup releases guarded workers from older builds. `intern_files.rs` binds approved writes to directory identities/handles and expected contents. `src/lib/components/intern/PiIntern.svelte` owns the floating chat, screenshot input, and approval cards. Intern is app-wide: it gets a project only when attached in its own picker, or automatically when opened while a thread is showing (that thread's project). Clearing Intern uses in-process `new_session` and remaps the coordinator via `replace_thread_session` (deliberately bypassing `adopt_state`'s same-session guard). See `docs/pi-intern.md` for scope, limits, cancellation, and recovery.

### Plan and Auto modes

`src-tauri/src/pidesk-modes.mjs` is loaded into every Pi πDesk spawns (`modes_arguments` in `threads.rs`; spawn fails closed without its `pidesk-mode` command). Plan blocks file-changing tools and non-allowlisted shell in a `tool_call` hook; `request_auto` shows an approval card and enables Auto until that run's `agent_end`. Threads default to Auto (`thread_modes` stores Plan) and switch live via `/pidesk-mode`; Pi Intern is locked to Plan. See `docs/modes.md`.

### Cross-cutting notes

- Only Pi is supported. The one-value `HarnessKind` wire type preserves IPC/schema compatibility and rejects unsupported agent values; it is not a selector. Do not advertise unverified Pi capabilities.
- Size/count limits are pervasive by design (frame bytes, history messages/bytes, cached sessions, pending frames, stderr tail, live thread count) — this is a long-running desktop app talking to external processes, so unbounded growth is treated as a bug class. Preserve existing caps and add new ones for any new unbounded buffer.
- Every Pi πDesk launches (threads, Intern, package installs, the terminal's `pi`) runs with a private home: `HOME=~/.pidesk/home` plus `XDG_*` under it (`util::private_home_env`, created by `prepare_private_home`, never during detection). Extensions resolve `~`/XDG from there, so their config, caches, and discovery (e.g. the MCP adapter's `~/.config/mcp`, `~/.claude`) stay inside `~/.pidesk`. The only link back is `home/Library/Keychains` (macOS keychain lookup needs it). `pidesk-modes.mjs` replaces the `bash` tool so commands the agent runs get the real `HOME` (from `PIDESK_REAL_HOME`) and default XDG, keeping git/gh/cargo/ssh identity working.
- Fresh private installs add `harness::DEFAULT_PACKAGES` (`npm:pi-mcp-adapter`) after verification; failure is logged, not fatal.
- Every new thread in a Git project (user- or Intern-created) gets its own detached worktree from HEAD; a repository with no commits yet, or a non-Git project, uses the project folder.
- Isolated worktrees live under `~/.pidesk/worktrees/`. Project removal only deletes metadata, never repo files, Git history, worktrees, or Pi session data.
- Naming: user-facing text says `πDesk`; code, crates, and paths use `pidesk`.
- App identity: `dev.pidesk.desktop`, database `pidesk.sqlite3`. πDesk is in beta and intentionally has no migration from earlier builds. Never import credentials, extensions, or sessions from the user's terminal Pi.
