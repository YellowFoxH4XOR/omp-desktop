import { expect, test } from 'vitest';
import { BUILTIN_COMMANDS, TERMINAL_ONLY, parseSlash } from './slash';

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
