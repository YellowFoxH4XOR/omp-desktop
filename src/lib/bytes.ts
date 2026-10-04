// Size accounting for untrusted backend data, so the app can bound what it keeps.

const utf8Encoder = new TextEncoder();

export function utf8Bytes(value: string): number {
  return utf8Encoder.encode(value).byteLength;
}
function jsonStringBytes(value: string, maxBytes: number): number {
  let bytes = 2;
  for (const character of value) {
    const code = character.codePointAt(0)!;
    if (code === 0x22 || code === 0x5c || code === 0x08 || code === 0x0c || code === 0x0a || code === 0x0d || code === 0x09) bytes += 2;
    else if (code < 0x20) bytes += 6;
    else if (code <= 0x7f) bytes += 1;
    else if (code <= 0x7ff) bytes += 2;
    else if (code <= 0xffff) bytes += 3;
    else bytes += 4;
    if (bytes > maxBytes) break;
  }
  return bytes;
}
/** Upper-bound size of `value` as JSON, without serializing it. Stops counting
 * once past `maxBytes`, so an oversized frame costs a bounded amount of work. */
export function estimateJsonBytes(value: unknown, maxBytes: number, seen = new WeakSet<object>()): number {
  let bytes = 0;
  let steps = 0;
  const visit = (current: unknown, depth = 0): void => {
    if (bytes > maxBytes || ++steps > 100_000 || depth > 512) { bytes = maxBytes + 1; return; }
    if (current === null) { bytes += 4; return; }
    if (typeof current === 'string') { bytes += jsonStringBytes(current, maxBytes); return; }
    if (typeof current === 'number') { bytes += 24; return; }
    if (typeof current === 'boolean') { bytes += current ? 4 : 5; return; }
    if (current === undefined || typeof current === 'function' || typeof current === 'symbol') return;
    if (typeof current !== 'object') { bytes += 8; return; }
    if (seen.has(current)) return;
    seen.add(current);
    if (Array.isArray(current)) {
      bytes += 2;
      for (let index = 0; index < current.length && bytes <= maxBytes; index++) {
        if (index) bytes += 1;
        visit(current[index], depth + 1);
      }
    } else {
      bytes += 2;
      let first = true;
      for (const key in current) {
        if (!Object.prototype.hasOwnProperty.call(current, key)) continue;
        if (!first) bytes += 1;
        first = false;
        bytes += jsonStringBytes(key, maxBytes) + 1;
        visit((current as Record<string, unknown>)[key], depth + 1);
        if (bytes > maxBytes) break;
      }
    }
  };
  visit(value);
  return bytes;
}
/** The longest prefix of `value` that fits in `maxBytes` of UTF-8. */
export function truncateUtf8(value: string, maxBytes: number): string {
  if (utf8Bytes(value) <= maxBytes) return value;
  let low = 0;
  let high = Math.min(value.length, maxBytes);
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (utf8Bytes(value.slice(0, middle)) <= maxBytes) low = middle;
    else high = middle - 1;
  }
  return value.slice(0, low);
}
export function errorText(error: unknown): string { return error instanceof Error ? error.message : String(error); }
