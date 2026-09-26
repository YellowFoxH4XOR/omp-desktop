# Plan and Auto modes

πDesk loads a bundled Pi extension (`src-tauri/src/pidesk-modes.mjs`, written to `~/.pidesk/extensions/pidesk-modes.mjs` on each spawn) into every Pi it starts. Spawn fails closed if its `pidesk-mode` command is missing.

| Mode | What Pi may do |
|---|---|
| **Auto** | Everything its tools allow. Default for project threads. |
| **Plan** | Read-only: `read`, `grep`, `find`, `ls`, `pidesk`, and allowlisted read-only shell commands (for example `cat`, `rg`, `git status/diff/log`, `curl` without upload/output flags). Edits, writes, other commands, MCP tool calls and unknown extension tools **ask first**: a `tool_call` hook shows a *Plan mode: allow this?* card describing the exact call, with **Allow once**, **Allow for this run (Auto)**, or **Keep blocked**. With no πDesk window to ask in, they stay blocked. MCP metadata lookups (`mcp({})`, `search`, `describe`, `instructions`) run freely. |

In Plan, the agent calls `request_auto` with a Markdown plan. πDesk recognises the request (`src/lib/plan.ts`) and opens **Plan review** (`components/plan/PlanReview.svelte`) automatically: the thread's right panel, or a sheet beside Pi Intern (opening Intern if hidden). It offers **Approve & run** (⌘↵), **Give feedback**, and **Decline**. Approval enables Auto until that run ends (`agent_end`); then Plan returns. Feedback is steered into the running turn first, then the request is answered *Revise with feedback*, so the agent revises and asks again. Hiding the panel leaves a *Review plan* bar. The transcript shows each `request_auto` call as a plan card with its outcome.

- **Threads:** the composer's Plan/Auto toggle stores the choice per thread (`thread_modes`; no row means Auto) and applies it to a running Pi immediately through the `/pidesk-mode` host command, which Pi handles without involving the model. `pidesk-*` commands are hidden from the slash menu.
- **Pi Intern:** always Plan and locked (`--pidesk-plan-locked`); the host command cannot switch it to Auto, so only an approval card can.
- **Shell allowlist:** every segment of a chained command must be allowlisted; command substitution and output redirection (other than `2>/dev/null` and `2>&1`) are rejected. It is deliberately conservative.
- **Checks:** `bun run test` runs `pidesk-modes.check.mjs`.
