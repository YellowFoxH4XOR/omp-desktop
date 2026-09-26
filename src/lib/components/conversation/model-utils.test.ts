import { expect, test } from 'vitest';
import { formatCost, formatTokens, matchesModel, providerLabel } from './model-utils';

test('provider labels are friendly and unknown ids are title-cased', () => {
  expect(providerLabel('opencode-go')).toBe('OpenCode Go');
  expect(providerLabel('openai-codex')).toBe('OpenAI Codex');
  expect(providerLabel('my_custom-endpoint')).toBe('My Custom Endpoint');
});

test('context windows and costs format compactly', () => {
  expect(formatTokens(262_144)).toBe('262K');
  expect(formatTokens(128_000)).toBe('128K');
  expect(formatTokens(1_048_576)).toBe('1M');
  expect(formatTokens(2_000_000)).toBe('2M');
  expect(formatTokens(undefined)).toBeUndefined();
  expect(formatCost({ input: 0.95, output: 4 })).toBe('$0.95 / $4');
  expect(formatCost({ input: 0, output: 0 })).toBe('Free');
});

test('search matches every term across name, id, and provider', () => {
  const kimi = { provider: 'opencode-go', id: 'kimi-k2.6', name: 'Kimi K2.6' };
  expect(matchesModel(kimi, 'kimi')).toBe(true);
  expect(matchesModel(kimi, 'go kimi')).toBe(true);
  expect(matchesModel(kimi, 'opencode k2')).toBe(true);
  expect(matchesModel(kimi, 'codex')).toBe(false);
});
