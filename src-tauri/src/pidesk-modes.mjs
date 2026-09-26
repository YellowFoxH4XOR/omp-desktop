// Bundled and explicitly loaded by πDesk into every Pi it starts. Adds Plan
// and Auto modes (in Plan, anything beyond reading asks the user first), and gives the agent's shell the user's real home back:
// πDesk runs Pi with a private HOME (~/.pidesk/home) so extensions keep their
// files inside ~/.pidesk, but git, gh, cargo, ssh… need the real one. Plan is read-only: file-changing tools and non-allowlisted
// shell commands are blocked here, in Pi, not by model instructions. The agent
// leaves Plan only when the user approves a request_auto card; Auto then lasts
// until that run ends and Plan returns.

// Pi's extension loader maps this to the running Pi (static imports only).
import { createBashTool } from '@earendil-works/pi-coding-agent';

const READ_ONLY_TOOLS = new Set(['read', 'grep', 'find', 'ls', 'pidesk', 'request_auto']);
// πDesk matches these labels (src/lib/plan.ts) to show its plan review panel.
const TITLE = 'Run this plan in Auto mode?';
const APPROVE = 'Approve and run in Auto';
const FEEDBACK = 'Revise with feedback';
const DECLINE = 'Decline';
const CONTEXT_TYPE = 'pidesk-plan-context';
const ASK_TITLE = 'Plan mode: allow this?';
const ALLOW_ONCE = 'Allow once';
const ALLOW_RUN = 'Allow for this run (Auto)';
const KEEP_BLOCKED = 'Keep blocked';
const ALLOW_SESSION = 'Allow for this session';
const PLAN_CONTEXT = `[πDesk PLAN MODE]
You are in Plan mode, which is read-only. You may read, search, list files, run read-only shell commands (for example cat, rg, ls, git status, git diff, git log), look things up on the web with curl, and look up MCP servers and tools with mcp({}), mcp({ search }) or mcp({ describe }). Edits, file writes, calling MCP tools, and every other command need the user's permission: πDesk asks them each time you try one, and they may allow it or keep it blocked.
For a single small step you may just try it. When the task needs several changes, first investigate, then present a concrete numbered plan with the exact files and commands involved, and call request_auto with that plan. If the user approves, Auto mode (full tools) is on until this run ends; carry out the plan in the same run, verify it, and report. If they decline, keep planning or ask what to change.`;

// A command passes only if every segment is an allowlisted read-only
// command. Substitution and output redirection are never trusted, except
// discarding output (`2>/dev/null`) or merging streams (`2>&1`).
const UNSAFE_SYNTAX = /`|\$\(|<\(|>/;
const DESTRUCTIVE = [
  /\b(rm|rmdir|mv|cp|mkdir|touch|chmod|chown|chgrp|ln|tee|truncate|dd|shred|sudo|su|kill|pkill|killall|reboot|shutdown)\b/i,
  /\s-(exec|execdir|delete|ok|fprint\w*)\b/,
  /\bsed\b.*\s-i/,
  /\bcurl\b.*\s(-o|-O|--output|--remote-name\S*|-T|--upload-file|-d|--data\S*|-F|--form|-X|--request)\b/,
  /\bgit\b.*\s--output\b/,
];
const SAFE = [
  /^cat\b/, /^head\b/, /^tail\b/, /^grep\b/, /^egrep\b/, /^rg\b/, /^fd\b/, /^find\b/, /^ls\b/, /^tree\b/,
  /^pwd$/, /^echo\b/, /^printf\b/, /^wc\b/, /^sort\b/, /^uniq\b/, /^cut\b/, /^tr\b/, /^diff\b/, /^file\b/,
  /^stat\b/, /^du\b/, /^df\b/, /^which\b/, /^whereis\b/, /^type\b/, /^printenv\b/, /^uname\b/,
  /^whoami$/, /^id\b/, /^date\b/, /^uptime$/, /^ps\b/, /^jq\b/, /^sed\s+-n\b/, /^bat\b/, /^eza\b/,
  /^basename\b/, /^dirname\b/, /^realpath\b/, /^readlink\b/, /^cd\b/, /^true$/,
  /^git\s+(status|log|diff|show|blame|shortlog|describe|rev-parse|ls-files|ls-tree|grep|reflog\s+show)\b/,
  /^git\s+branch(\s+(-a|-r|-v|-vv|--list|--all|--remotes|--show-current|--merged|--no-merged|--contains\s+\S+))*$/,
  /^git\s+remote(\s+-v)?$/,
  /^git\s+config\s+--get\b/,
  /^git\s+stash\s+list\b/,
  /^(npm|pnpm)\s+(ls|list|view|info|outdated|why)\b/,
  /^yarn\s+(list|info|why)\b/,
  /^(node|python3?|bun|cargo|rustc|go|npm|pnpm|yarn)\s+(--version|-V)$/,
  /^cargo\s+(metadata|tree)\b/,
  /^curl\s/,
];

/** pi-mcp-adapter's `mcp` calls that only read metadata: status, a server's
 *  tool list, search, describe, instructions. Calling a tool, connecting,
 *  and auth actions still need Auto. */
export function isMcpLookup(input) {
  if (!input || typeof input !== 'object') return true;
  return !['tool', 'connect', 'action', 'args', 'code', 'script'].some(key => key in input && input[key] !== undefined && input[key] !== null && input[key] !== '');
}

// Other apps' config and credential locations under the user's real home.
// The agent's shell sees that home, so reaching into these asks first.
const FOREIGN = ['.pi', '.config/mcp', '.agents', '.claude.json', '.claude', '.cursor', '.codex', '.windsurf', '.config/opencode', '.gemini', 'Library/Application Support/Claude', '.ssh', '.aws', '.config/gcloud', '.netrc', '.npmrc', '.git-credentials'];
const FOREIGN_TOOLS = new Set(['read', 'grep', 'find', 'ls', 'bash']);

/** The other app's location this call reaches into, e.g. `~/.config/mcp`, or null. */
export function foreignPath(toolName, input, realHome) {
  if (!FOREIGN_TOOLS.has(toolName) || !input || typeof input !== 'object') return null;
  const text = toolName === 'bash'
    ? String(input.command ?? '')
    : [input.path, input.file_path, input.pattern].filter(value => typeof value === 'string').join('\n');
  if (!text) return null;
  const homes = ['~', '$HOME', '${HOME}', ...(realHome ? [realHome.replace(/\/+$/, '')] : [])];
  for (const rel of FOREIGN) {
    for (const home of homes) {
      let from = 0;
      for (;;) {
        const at = text.indexOf(`${home}/${rel}`, from);
        if (at < 0) break;
        const next = text[at + home.length + 1 + rel.length];
        if (next === undefined || /[\/\s'"`;|&)]/.test(next)) return `~/${rel}`;
        from = at + 1;
      }
    }
  }
  return null;
}

/** What a blocked call would do, for the permission card. */
export function describeCall(toolName, input) {
  const text = value => (typeof value === 'string' ? value : '');
  const clip = (value, max) => (value.length > max ? `${value.slice(0, max)}…` : value);
  const path = text(input?.path) || text(input?.file_path);
  if (toolName === 'bash') return `Run in the shell:\n${clip(text(input?.command), 1200)}`;
  if (toolName === 'read') return `Read ${path || 'a file'}`;
  if (toolName === 'grep' || toolName === 'find' || toolName === 'ls') {
    const pattern = text(input?.pattern);
    return `${toolName === 'ls' ? 'List' : 'Search'} ${path || text(input?.path) || 'files'}${pattern ? ` for “${clip(pattern, 120)}”` : ''}`;
  }
  if (toolName === 'edit') {
    const edits = Array.isArray(input?.edits) ? input.edits.length : 1;
    return `Edit ${path || 'a file'} (${edits} change${edits === 1 ? '' : 's'})`;
  }
  if (toolName === 'write') return `Write ${path || 'a file'} (${text(input?.content).length} characters)`;
  if (toolName === 'mcp') {
    const target = text(input?.tool) || text(input?.connect) || text(input?.action);
    return `Use MCP: ${target || 'call'}${input?.args ? `\n${clip(JSON.stringify(input.args), 600)}` : ''}`;
  }
  let args = '';
  try { args = JSON.stringify(input ?? {}); } catch { args = ''; }
  return `Use ${toolName}${args && args !== '{}' ? `\n${clip(args, 600)}` : ''}`;
}

export function isReadOnlyCommand(command) {
  if (typeof command !== 'string' || !command.trim() || command.length > 8192) return false;
  const cleaned = command.replace(/\s\d?>\s*\/dev\/null(?=\s|$|[;|&])/g, ' ').replace(/\s\d>&\d(?=\s|$|[;|&])/g, ' ');
  if (UNSAFE_SYNTAX.test(cleaned)) return false;
  const segments = cleaned.split(/\|\||&&|[;|\n&]/).map(segment => segment.trim()).filter(Boolean);
  return segments.length > 0 && segments.every(segment =>
    !DESTRUCTIVE.some(pattern => pattern.test(segment)) && SAFE.some(pattern => pattern.test(segment)));
}

const PRIVATE_HOME_KEYS = ['XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'PIDESK_REAL_HOME'];

/** Environment for a command the agent runs: the real home, default XDG. */
export function realHomeEnv(env, realHome) {
  const next = { ...env, HOME: realHome };
  for (const key of PRIVATE_HOME_KEYS) delete next[key];
  return next;
}

/** Pi's bash tool, with each command spawned under the real home. */
export function realHomeBash(createBashTool, cwd, realHome) {
  const tool = createBashTool(cwd, {
    spawnHook: ctx => ({ ...ctx, env: realHomeEnv(ctx.env ?? process.env, realHome) }),
  });
  return { ...tool, execute: (id, params, signal, onUpdate) => tool.execute(id, params, signal, onUpdate) };
}

export default function (pi) {
  let plan = false;
  let locked = false;
  let executing = false;
  const allowedForeign = new Set();
  const realHome = process.env.PIDESK_REAL_HOME;

  function status(ctx) {
    ctx?.ui?.setStatus?.('pidesk-mode', plan ? (executing ? 'plan:approved' : 'plan') : 'auto');
  }

  pi.registerFlag('pidesk-plan', { description: 'Start in πDesk Plan mode (read-only)', type: 'boolean', default: false });
  pi.registerFlag('pidesk-plan-locked', { description: 'Keep πDesk Plan mode on; Auto only through approval', type: 'boolean', default: false });

  pi.registerCommand('pidesk-mode', {
    description: 'πDesk host control: set plan or auto',
    handler: async (args, ctx) => {
      const mode = String(args ?? '').trim();
      if (mode !== 'plan' && mode !== 'auto') return;
      if (mode === 'auto' && locked) return;
      plan = mode === 'plan';
      executing = false;
      status(ctx);
    },
  });

  pi.registerTool({
    name: 'request_auto',
    label: 'Request Auto mode',
    description: `Only needed in Plan mode. Call it when your plan needs changes (edits, writes, or commands beyond read-only inspection). Pass the plan as Markdown: a short goal line, then numbered steps naming exact files and commands, then how you will verify. πDesk shows it in a review panel where the user approves, declines, or sends feedback. If approved, full tools are on until this run ends; carry out the plan immediately.`,
    parameters: {
      type: 'object',
      properties: { plan: { type: 'string', description: 'The numbered steps you will carry out, naming files and commands.' } },
      required: ['plan'],
      additionalProperties: false,
    },
    async execute(_id, params, signal, _update, ctx) {
      const text = value => ({ content: [{ type: 'text', text: value }], details: undefined });
      if (!plan) return text('Auto mode is already on. Proceed.');
      if (executing) return text('Already approved. Auto mode is on until this run ends. Proceed.');
      if (!ctx.hasUI) throw new Error('Plan approval needs the πDesk host.');
      const steps = String(params?.plan ?? '').trim().slice(0, 6000);
      if (!steps) throw new Error('Pass the plan you want approved.');
      const choice = await ctx.ui.select(`${TITLE}\n\n${steps}`, [APPROVE, FEEDBACK, DECLINE], { signal });
      if (choice === FEEDBACK) return text('The user wants changes. Their feedback arrives as the next message: revise the plan to address it, then call request_auto again with the revised plan.');
      if (choice !== APPROVE) return text('The user declined. Stay in Plan mode: ask what they want instead. Do not retry the same request.');
      executing = true;
      status(ctx);
      return text('Approved. Auto mode is on until this run ends, then Plan mode returns. Carry out the plan now, then verify and report.');
    },
  });

  // Reads pass. Anything else asks the user, who can allow it once, allow
  // the rest of this run (Auto until it ends), or keep it blocked. Without a
  // πDesk window to ask in, it stays blocked.
  pi.on('tool_call', async (event, ctx) => {
    // In either mode, reaching into another app's config or credentials asks.
    const foreign = foreignPath(event.toolName, event.input, realHome);
    if (foreign && !allowedForeign.has(foreign)) {
      const declined = { block: true, reason: `${foreign} belongs to another app and may hold credentials. The user did not allow reading it; don't try again. Your own Pi profile is $PI_CODING_AGENT_DIR (~/.pidesk/agent).` };
      if (!ctx?.hasUI) return declined;
      const choice = await ctx.ui.select(
        `Read another app's files?\n\n${describeCall(event.toolName, event.input)}\n\n${foreign} belongs to another app, not πDesk, and may hold credentials. Anything read is sent to your model provider.`,
        [ALLOW_ONCE, ALLOW_SESSION, KEEP_BLOCKED],
        { signal: ctx.signal },
      );
      if (choice === ALLOW_SESSION) allowedForeign.add(foreign);
      else if (choice !== ALLOW_ONCE) return declined;
    }
    if (!plan || executing || READ_ONLY_TOOLS.has(event.toolName)) return undefined;
    if (event.toolName === 'mcp' && isMcpLookup(event.input)) return undefined;
    if (event.toolName === 'bash' && isReadOnlyCommand(event.input?.command)) return undefined;
    if (!ctx?.hasUI) {
      return { block: true, reason: `Plan mode is read-only and no one could be asked, so ${event.toolName} was blocked. Put this step in a plan and call request_auto.` };
    }
    const choice = await ctx.ui.select(`${ASK_TITLE}\n\n${describeCall(event.toolName, event.input)}`, [ALLOW_ONCE, ALLOW_RUN, KEEP_BLOCKED], { signal: ctx.signal });
    if (choice === ALLOW_ONCE) return undefined;
    if (choice === ALLOW_RUN) {
      executing = true;
      status(ctx);
      return undefined;
    }
    return { block: true, reason: `The user kept this ${event.toolName} call blocked in Plan mode. Do not retry it; keep planning, or ask what they want instead.` };
  });

  pi.on('user_bash', () => {
    if (plan && !executing) throw new Error('Plan mode is read-only. Switch the thread to Auto to run commands.');
    return undefined;
  });

  pi.on('before_agent_start', () => plan && !executing
    ? { message: { customType: CONTEXT_TYPE, content: PLAN_CONTEXT, display: false } }
    : undefined);

  // Keep only the newest Plan reminder in context, and none while in Auto.
  pi.on('context', event => {
    const keep = plan && !executing;
    let seen = false;
    const messages = [...event.messages].reverse().filter(message => {
      if (message?.customType !== CONTEXT_TYPE) return true;
      if (!keep || seen) return false;
      seen = true;
      return true;
    }).reverse();
    return messages.length === event.messages.length ? undefined : { messages };
  });

  pi.on('agent_end', (_event, ctx) => {
    if (!executing) return;
    executing = false;
    status(ctx);
  });

  pi.on('session_start', (_event, ctx) => {
    locked = pi.getFlag('pidesk-plan-locked') === true;
    plan = locked || pi.getFlag('pidesk-plan') === true;
    executing = false;
    status(ctx);
  });

  // Last, after every hook above is registered synchronously.
  if (realHome) pi.registerTool(realHomeBash(createBashTool, process.cwd(), realHome));
}
