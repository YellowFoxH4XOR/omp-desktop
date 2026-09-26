import { expect, test } from 'vitest';
import { planFacts, planOf, planOutcome } from './plan';

test('plan facts count numbered steps, distinct files, and the first command', () => {
  const plan = '## Toggle\n\n1. Edit `src/App.svelte`\n2. Edit `src/app.css` and `src/App.svelte`\n3) Run `bun test`\n- not a step\n\n`theme` is a key, not a file.';
  expect(planFacts(plan)).toEqual({ steps: 3, files: 2, command: 'bun test' });
  expect(planFacts('Nothing to count')).toEqual({ steps: 0, files: 0, command: undefined });
});

test('only the modes extension approval is treated as a plan', () => {
  const base = { id: '1', method: 'select' as const, options: ['Approve and run in Auto', 'Revise with feedback', 'Decline'] };
  expect(planOf({ ...base, title: 'Run this plan in Auto mode?\n\n1. Do it' })).toBe('1. Do it');
  expect(planOf({ ...base, title: 'Pick one' })).toBeNull();
  expect(planOf({ ...base, title: 'Run this plan in Auto mode?\n\nx', options: ['Yes'] })).toBeNull();
  expect(planOutcome('Approved. Auto mode is on')).toBe('approved');
  expect(planOutcome('The user wants changes. Their feedback')).toBe('revising');
  expect(planOutcome('The user declined.')).toBe('declined');
});
