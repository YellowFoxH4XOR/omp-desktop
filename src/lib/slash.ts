/** Where a `/` command comes from. */
export type SlashSource = 'pidesk' | 'extension' | 'skill' | 'prompt' | 'terminal';

export interface SlashCommand {
  name: string;
  description?: string;
  /** Argument hint, e.g. `[provider/model]`. */
  args?: string;
  source: SlashSource;
}

/**
 * Pi's built-in terminal commands don't exist over RPC; sent as a prompt they
 * would reach the model as text. πDesk runs these itself.
 */
export const BUILTIN_COMMANDS: SlashCommand[] = [
  { name: 'model', args: '[provider/model]', description: 'Switch model, or open the model picker', source: 'pidesk' },
  { name: 'thinking', args: '[level]', description: 'Set reasoning effort (off, low, medium, high…)', source: 'pidesk' },
  { name: 'compact', args: '[instructions]', description: 'Summarize older context to free up tokens', source: 'pidesk' },
  { name: 'new', description: 'Start a new thread in this project', source: 'pidesk' },
  { name: 'name', args: '<title>', description: 'Rename this thread', source: 'pidesk' },
  { name: 'plan', description: 'Switch this thread to Plan mode (read-only)', source: 'pidesk' },
  { name: 'auto', description: 'Switch this thread to Auto mode (full tools)', source: 'pidesk' },
  { name: 'session', description: 'Show tokens, cost, and context use', source: 'pidesk' },
  { name: 'copy', description: 'Copy the last reply', source: 'pidesk' },
  { name: 'reload', description: "Restart this thread's Pi to reload extensions, skills and settings", source: 'pidesk' },
  { name: 'login', description: 'Sign in to a model provider in the Pi terminal', source: 'pidesk' },
];

/** Built-ins that only make sense in Pi's own terminal UI. */
export const TERMINAL_ONLY = new Set([
  'settings', 'scoped-models', 'logout', 'resume', 'tree', 'fork', 'clone', 'import', 'export', 'share', 'bug', 'trust', 'hotkeys', 'changelog', 'quit', 'exit',
]);

/** `/name rest of line` → { name, args }; null for ordinary messages. */
export function parseSlash(message: string): { name: string; args: string } | null {
  const match = /^\/([A-Za-z0-9][\w:.-]*)(?:\s+([\s\S]*))?$/.exec(message.trim());
  return match ? { name: match[1], args: (match[2] ?? '').trim() } : null;
}

/** Built-ins that apply to Pi Intern: it's always in Plan and has no title. */
export const INTERN_COMMANDS: SlashCommand[] = BUILTIN_COMMANDS
  .filter(command => !['name', 'plan', 'auto'].includes(command.name))
  .map(command => command.name === 'new' ? { ...command, description: 'Start a new Intern conversation' } : command);

/**
 * Menu entries for `/…` typed at the start of a message: built-ins first,
 * then Pi's own commands (never πDesk's `pidesk-*` host controls). Names
 * starting with the query come before names or descriptions containing it.
 */
export function slashSuggestions(
  text: string,
  commands: Array<{ name: string; description?: string; source?: 'extension' | 'skill' | 'prompt' }>,
  builtins: SlashCommand[] = BUILTIN_COMMANDS,
  limit = 10,
): SlashCommand[] {
  if (!text.startsWith('/') || /\s/.test(text)) return [];
  const own = new Set(builtins.map(command => command.name));
  const all: SlashCommand[] = [
    ...builtins,
    ...commands
      .filter(command => !command.name.startsWith('pidesk-') && !own.has(command.name))
      .map(command => ({ name: command.name, description: command.description, source: command.source ?? 'extension' }) as SlashCommand),
  ];
  const needle = text.slice(1).toLowerCase();
  const starts = all.filter(command => command.name.toLowerCase().startsWith(needle));
  const contains = needle
    ? all.filter(command => !command.name.toLowerCase().startsWith(needle) && (command.name.toLowerCase().includes(needle) || command.description?.toLowerCase().includes(needle)))
    : [];
  return [...starts, ...contains].slice(0, limit);
}

export const SOURCE_LABEL: Record<SlashSource, string> = { pidesk: 'πDesk', extension: 'Extension', skill: 'Skill', prompt: 'Prompt', terminal: 'Terminal' };
