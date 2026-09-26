# Repository Guidelines

## Project Structure & Module Organization

πDesk is a macOS Tauri 2 app with a Svelte 5 and TypeScript frontend in `src/` and a Rust backend in `src-tauri/src/`. `src/App.svelte` owns the app shell; `src/lib/api.ts` is the typed Tauri bridge, and `src/lib/session.svelte.ts` manages conversation state. Group UI work under `src/lib/components/` by feature. Frontend unit tests sit beside their modules as `*.test.ts`; browser journeys live in `tests/e2e/`, and Rust integration tests in `src-tauri/tests/`. Static assets are in `public/` and `src-tauri/icons/`; architecture notes are in `docs/`.

## Build, Test, and Development Commands

- `bun install --frozen-lockfile`: install the locked dependencies.
- `bun run tauri dev`: start the desktop app with the Vite frontend and Rust backend.
- `bun run check`: run Svelte/TypeScript checks, Rust formatting validation, and `cargo check`.
- `bun run test`: run Vitest unit tests and the Node extension checks.
- `bunx playwright install chromium && bun run test:e2e`: run mocked desktop journeys.
- `bun run test:rust`: run Rust tests; `bun run build` builds the frontend.
- `bun run validate`: run the full CI sequence, including the macOS app bundle on macOS.

## Coding Style & Naming Conventions

Use the surrounding file's formatting: frontend code generally uses two-space indentation; Rust uses `rustfmt` and four spaces. Keep Svelte components in `PascalCase.svelte`, TypeScript helpers and tests in descriptive lowercase names, and Rust modules in `snake_case.rs`. TypeScript is checked in strict mode. There is no separate frontend lint or formatting script; run `bun run check` before submitting.

## Testing Guidelines

Add focused `*.test.ts` tests for frontend logic, `*.spec.ts` tests for browser flows, and Rust tests near the affected code or in `src-tauri/tests/`. Playwright runs against a mocked Tauri bridge. No numeric coverage threshold is configured; cover changed behavior and relevant failure cases. Live Pi RPC tests are ignored by default and require an installed private Pi.

## Commits & Pull Requests

Recent commits use short, imperative summaries such as “Add thread delete…” and “Harden race handling…”. Keep each commit focused. In pull requests, explain the behavior change, link a relevant issue when one exists, list validation performed, and include screenshots for visible UI changes. CI runs checks, tests, the frontend build, and a macOS bundle build.

## Security & Local Data

Keep credentials out of commits and logs. πDesk uses its private Pi installation and profile under `~/.pidesk/`; preserve that separation when changing runtime or configuration code.
