import { describe, expect, test } from 'vitest';
import { BRAND_ICONS } from './brand-icons';
import { modelIcon, providerIcon } from './brands';

describe('brand icons', () => {
  test('providers map to their logos, including prefixed ids', () => {
    expect(providerIcon('github-copilot')).toBe('githubcopilot');
    expect(providerIcon('opencode-go')).toBe('opencode');
    expect(providerIcon('openai-codex')).toBe('openai');
    expect(providerIcon('google-vertex')).toBe('vertexai-color');
    expect(providerIcon('someone-new')).toBeUndefined();
  });
  test('models show their maker, whoever hosts them', () => {
    expect(modelIcon({ id: 'claude-sonnet-5' })).toBe('claude-color');
    expect(modelIcon({ id: 'opus' })).toBe('claude-color');
    expect(modelIcon({ id: 'gpt-5.3-codex' })).toBe('openai');
    expect(modelIcon({ id: 'o3-mini' })).toBe('openai');
    expect(modelIcon({ id: 'gemini-3.5-flash' })).toBe('gemini-color');
    expect(modelIcon({ id: 'kimi-k2.6' })).toBe('kimi-color');
    expect(modelIcon({ id: 'glm-5.1' })).toBe('zhipu-color');
    expect(modelIcon({ id: 'longcat-2.5-preview-free' })).toBe('longcat-color');
    expect(modelIcon({ id: 'mystery-1' })).toBeUndefined();
  });
  test('vendored markup is inert SVG', () => {
    for (const svg of Object.values(BRAND_ICONS)) {
      expect(svg.startsWith('<svg aria-hidden="true" ')).toBe(true);
      expect(svg).not.toMatch(/<script|\son\w+=|javascript:|<foreignObject/i);
    }
  });
});
