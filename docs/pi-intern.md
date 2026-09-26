# Pi Intern

Pi Intern is a non-modal chat opened by **Intern** beside Changes. It floats above the resource monitor. Hiding it does not stop work; **Stop Intern** cancels pending approvals, drains in-flight app operations, and stops the coordinator. Project threads Intern started keep running; stop them like any thread. It starts only when explicitly opened. **Clear conversation** (eraser) aborts the coordinator's turn, revokes its pending plans, and starts a fresh Pi session in the same process (RPC `new_session`); project threads are untouched, and the old journal stays on disk unmapped.

Intern is app-wide. It sees a project only when one is attached in its picker, or when opened while a thread is showing (that thread's project is attached).

## Capabilities

- Pi's built-in tools (`read`, `bash`, `edit`, `write`, and any enabled `grep`/`find`/`ls`) plus every extension, skill and prompt template installed in the private Pi profile. Its instructions are appended to Pi's default system prompt.
- It is locked to **Plan mode** (see `docs/modes.md`), enforced inside Pi by the bundled modes extension: reads, searches, read-only shell commands and `curl` lookups run freely; edits, writes and other commands are blocked. To change anything it calls `request_auto` with a numbered plan, which shows an approval card in the Intern panel. Approval turns on Auto for the rest of that run only.
- It works from its private folder (`~/.pidesk/intern`); each message states the attached project's absolute path, which it uses for files and commands.

- Inspect πDesk projects, the selected project's threads, private Pi metadata and installed documentation.
- Read bounded text and propose exact file/configuration changes.
- Propose adding/removing projects, renaming/pinning/archiving threads, and managing their lifecycle.
- Start ordinary project threads (full Pi: shell, edits, tests) through an approved plan, read their recent text, and send any thread in the attached project prompt/steer/follow-up messages through approved actions. A started thread gets its own Git worktree in a Git project (the project folder otherwise), exactly like a thread the user creates.
- Attach/paste up to four PNG/JPEG/WebP screenshots; images are resized locally and validated again by Rust. Images and inspected content are sent to the configured model provider. Image-capable models are required.

The header identifies the project for the **next message**; each approval displays its own captured project path. Changing the selected project does not silently retarget an existing plan.

## Approval boundary

The coordinator runs the private Pi executable over JSONL RPC with Pi's tools and extensions; `intern_threads` marks it so πDesk loads the bundled bridge and checks its health command. The `pidesk` tool's app actions still go through host-validated approval plans. Threads it starts are not guarded: once the plan that creates or messages them is approved, they act with Pi's full tools without per-action Intern approval. Guarded workers from earlier builds are released to ordinary threads at startup. The explicit bundled extension exposes only the `pidesk` bridge; discovered resources and built-in tools are disabled. Startup fails closed if the guard does not register its health command.

Rust validates requests and retains immutable action batches. Only the native `intern_approve` command can release a one-shot approval. Model text, screenshot contents and thread replies cannot approve a plan. Approvals expire, are cancelled on stop/abort/project removal, and cannot transfer to a replacement process. File writes retain directory identities and handles and use no-follow descriptor-relative access and macOS atomic replacement. A changed target/content requires a new plan.

A batch stops on its first failure. Earlier completed actions are **not rolled back**. Further actions require fresh approval. Permanent thread deletion remains in the manual thread menu; Intern can propose archiving instead.

## Deliberate limits

- The coordinator's edits and commands need a Plan → Auto approval; the read-only command allowlist is conservative, so some harmless commands are blocked in Plan. It must not claim checks passed without evidence.
- Permanent thread deletion remains a manual app action, not an Intern tool.
- Home/root scopes and known credential/application-data folders are not Intern project capabilities. Choose a dedicated project directory.
- Starting a thread while another is busy in a non-Git project fails, as it does for user-created threads.
- Private Pi credentials, the terminal Pi profile, session storage, and Intern control files are excluded from the `pidesk` file operations (Pi's own `read` is not restricted; the instructions forbid reading credentials). Private `models.json` is excluded because it may contain inline credentials; use Settings/manual configuration for those secrets. Project files and screenshots can still contain secrets—review what you share.
- Text files are limited to 64 KiB. Thread reports contain at most 24 text messages, 4 KiB each; oversized journals fail explicitly rather than returning a stale prefix.
- A broken private runtime or missing provider login may prevent Intern itself from starting. Setup/Settings and the manual terminal remain the recovery path. No repair success is guaranteed.

## Checks

`bun run check`, `bun run test` (includes the host-only extension checks), `bun run test:rust`, `bun run test:e2e`, and `bun run bundle:macos`.

The private Pi guard can be smoke-tested via `get_commands` without a model prompt. Provider-backed orchestration is not part of the default tests.
