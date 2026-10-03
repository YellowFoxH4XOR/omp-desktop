import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';

// The e2e journeys run against a mock of the Tauri IPC, so nothing there
// notices when a Rust command is renamed or its arguments change. These
// checks read the sources on both sides and fail on drift.

const read = (path: string) => readFileSync(path, 'utf8');
const camel = (name: string) => name.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());

/** Values Tauri injects; the frontend never sends them. */
const INJECTED = /^(tauri::)?(State|AppHandle|Window|WebviewWindow)\b/;

interface RustCommand { required: string[]; optional: string[] }

function rustCommands(): Map<string, RustCommand> {
  const commands = new Map<string, RustCommand>();
  const pattern = /#\[tauri::command\]\s*pub\s+(?:async\s+)?fn\s+(\w+)\s*\(([^)]*)\)/g;
  for (const [, name, params] of read('src-tauri/src/commands.rs').matchAll(pattern)) {
    const command: RustCommand = { required: [], optional: [] };
    // Split on commas outside generics: `State<'_, AppState>` is one parameter.
    let depth = 0;
    let current = '';
    const parts: string[] = [];
    for (const character of params) {
      if (character === '<') depth += 1;
      if (character === '>') depth -= 1;
      if (character === ',' && depth === 0) {
        parts.push(current);
        current = '';
      } else {
        current += character;
      }
    }
    parts.push(current);
    for (const part of parts) {
      const match = /^\s*(?:mut\s+)?(\w+)\s*:\s*(.+?)\s*$/s.exec(part);
      if (!match || INJECTED.test(match[2])) continue;
      (match[2].startsWith('Option<') ? command.optional : command.required).push(camel(match[1]));
    }
    commands.set(name, command);
  }
  return commands;
}

function registeredCommands(): Set<string> {
  const source = read('src-tauri/src/lib.rs');
  const start = source.indexOf('generate_handler![');
  const handler = source.slice(start, source.indexOf('])', start));
  return new Set([...handler.matchAll(/commands::(\w+)/g)].map((match) => match[1]));
}

interface FrontendCall { always: string[]; sometimes: string[] }

function frontendCalls(): Map<string, FrontendCall> {
  const calls = new Map<string, FrontendCall>();
  for (const line of read('src/lib/api.ts').split('\n')) {
    const match = /=>\s*invoke\b.*?\('(\w+)'(.*)$/.exec(line);
    if (!match) continue;
    const call: FrontendCall = { always: [], sometimes: [] };
    const open = match[2].indexOf('{');
    if (open >= 0) {
      // Keys at depth 1 are always sent; keys inside a spread only sometimes.
      // A key is the first identifier after `{` or `,`; values are skipped.
      let depth = 0;
      let expectKey = false;
      for (const token of match[2].slice(open).matchAll(/[{},]|[A-Za-z_]\w*/g)) {
        if (token[0] === '{') {
          depth += 1;
          expectKey = true;
        } else if (token[0] === '}') {
          depth -= 1;
          expectKey = false;
          if (depth === 0) break;
        } else if (token[0] === ',') {
          expectKey = true;
        } else if (expectKey) {
          (depth === 1 ? call.always : call.sometimes).push(token[0]);
          expectKey = false;
        }
      }
    }
    calls.set(match[1], call);
  }
  return calls;
}

describe('frontend ↔ Rust command contract', () => {
  const rust = rustCommands();
  const registered = registeredCommands();
  const frontend = frontendCalls();

  it('parses both sides', () => {
    expect(rust.size).toBeGreaterThan(40);
    expect(frontend.size).toBeGreaterThan(40);
  });

  it('registers every #[tauri::command], and only those', () => {
    expect([...registered].sort()).toEqual([...rust.keys()].sort());
  });

  it('calls only registered commands, and leaves none unused', () => {
    expect([...frontend.keys()].sort()).toEqual([...registered].sort());
  });

  it('sends the arguments each command takes', () => {
    const problems: string[] = [];
    for (const [name, call] of frontend) {
      const command = rust.get(name);
      if (!command) continue;
      const accepted = new Set([...command.required, ...command.optional]);
      for (const key of [...call.always, ...call.sometimes])
        if (!accepted.has(key)) problems.push(`${name}: sends "${key}", which Rust does not take`);
      for (const key of command.required)
        if (!call.always.includes(key)) problems.push(`${name}: never sends required "${key}"`);
    }
    expect(problems).toEqual([]);
  });

  it('mocks only real commands in the e2e journeys', () => {
    const mocked = [...read('tests/e2e/desktop-journey.spec.ts').matchAll(/command === '([^']+)'/g)]
      .map((match) => match[1])
      .filter((name) => !name.startsWith('plugin:'));
    expect(mocked.length).toBeGreaterThan(20);
    expect([...new Set(mocked)].filter((name) => !registered.has(name))).toEqual([]);
  });
});
