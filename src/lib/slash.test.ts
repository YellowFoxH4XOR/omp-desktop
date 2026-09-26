import { expect, test } from 'vitest';
import { BUILTIN_COMMANDS, INTERN_COMMANDS, TERMINAL_ONLY, parseSlash, slashSuggestions } from './slash';

test('slash commands parse into a name and the rest of the line', () => {
  expect(parseSlash('/model opencode-go/glm-5.1')).toEqual({ name: 'model', args: 'opencode-go/glm-5.1' });
  expect(parseSlash('  /compact  keep the API notes\nand tests ')).toEqual({ name: 'compact', args: 'keep the API notes\nand tests' });
  expect(parseSlash('/skill:mcp-scripting')).toEqual({ name: 'skill:mcp-scripting', args: '' });
  expect(parseSlash('please /compact')).toBeNull();
  expect(parseSlash('/')).toBeNull();
  // A path is not a command.
  expect(parseSlash('/usr/bin is a path')).toBeNull();
});

test('πDesk handles the common built-ins and knows which are terminal-only', () => {
  const names = BUILTIN_COMMANDS.map(command => command.name);
  for (const name of ['model', 'thinking', 'compact', 'new', 'reload', 'login']) expect(names).toContain(name);
  expect(TERMINAL_ONLY.has('tree')).toBe(true);
  expect(names.some(name => TERMINAL_ONLY.has(name))).toBe(false);
});

test('suggestions list built-ins, then Pi commands, never host controls', () => {
  const pi = [{ name: 'mcp-auth', description: 'Sign in to an MCP server', source: 'extension' as const }, { name: 'pidesk-mode' }, { name: 'skill:mcp-scripting', source: 'skill' as const }];
  expect(slashSuggestions('/', pi, BUILTIN_COMMANDS, 50).map(command => command.name)).toContain('mcp-auth');
  expect(slashSuggestions('/', pi, BUILTIN_COMMANDS, 50).map(command => command.name)).not.toContain('pidesk-mode');
  expect(slashSuggestions('/mc', pi).map(command => command.name)).toEqual(['mcp-auth', 'skill:mcp-scripting']);
  expect(slashSuggestions('/model x', pi)).toEqual([]);
  expect(slashSuggestions('hello', pi)).toEqual([]);
  // Intern has no titles or modes to switch.
  expect(INTERN_COMMANDS.map(command => command.name)).not.toContain('plan');
  expect(INTERN_COMMANDS.find(command => command.name === 'new')?.description).toMatch(/Intern conversation/);
});
