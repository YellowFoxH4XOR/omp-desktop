import { describe, expect, test } from 'vitest';
import { errorText, estimateJsonBytes, truncateUtf8, utf8Bytes } from './bytes';

describe('byte accounting', () => {
  test('counts UTF-8 bytes, not characters', () => {
    expect(utf8Bytes('abc')).toBe(3);
    expect(utf8Bytes('π')).toBe(2);
    expect(utf8Bytes('😀')).toBe(4);
  });
  test('the estimate is never below the real JSON size', () => {
    const samples: unknown[] = [
      null, true, false, 'plain', 'quote " and \\ slash\n', 'π😀', '\u0001',
      [1, 'two', [3]], { a: 1, b: { c: ['d', null] } }, [],
    ];
    for (const sample of samples)
      expect(estimateJsonBytes(sample, 1_000_000)).toBeGreaterThanOrEqual(utf8Bytes(JSON.stringify(sample)));
  });
  test('stops once past the budget, including on cycles and deep nesting', () => {
    expect(estimateJsonBytes('x'.repeat(10_000), 100)).toBeGreaterThan(100);
    const cycle: Record<string, unknown> = { name: 'loop' };
    cycle.self = cycle;
    expect(estimateJsonBytes(cycle, 1_000)).toBeLessThanOrEqual(1_000);
    let deep: unknown = 'leaf';
    for (let depth = 0; depth < 600; depth += 1) deep = [deep];
    expect(estimateJsonBytes(deep, 1_000_000)).toBeGreaterThan(1_000_000);
  });
  test('truncates on a character boundary within the byte limit', () => {
    expect(truncateUtf8('hello', 10)).toBe('hello');
    expect(truncateUtf8('hello', 3)).toBe('hel');
    expect(utf8Bytes(truncateUtf8('πππππ', 5))).toBeLessThanOrEqual(5);
    expect(truncateUtf8('πππππ', 5)).toBe('ππ');
  });
  test('describes any thrown value', () => {
    expect(errorText(new Error('boom'))).toBe('boom');
    expect(errorText('plain')).toBe('plain');
  });
});
