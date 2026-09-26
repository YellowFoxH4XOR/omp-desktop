import type { ConversationItem, RpcContent, ToolResult } from '../../types';

export type ToolItem = Extract<ConversationItem, { kind: 'tool' }>;

const PATH_KEYS = ['path', 'file_path', 'filePath', 'filename', 'file', 'target_file', 'targetFile'];
const COMMAND_KEYS = ['command', 'cmd', 'script', 'code'];
const QUERY_KEYS = ['pattern', 'query', 'regex', 'search', 'term', 'q'];
const URL_KEYS = ['url', 'uri', 'link'];

export function firstString(source: Record<string, unknown>, keys: string[]): string | undefined {
  for (const key of keys) {
    const value = source[key];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return undefined;
}

export function argPath(args: Record<string, unknown>): string | undefined {
  return firstString(args, PATH_KEYS);
}

export function argCommand(args: Record<string, unknown>): string | undefined {
  return firstString(args, COMMAND_KEYS);
}

export function argQuery(args: Record<string, unknown>): string | undefined {
  return firstString(args, QUERY_KEYS);
}

export function argUrl(args: Record<string, unknown>): string | undefined {
  return firstString(args, URL_KEYS);
}

function detailRecord(details: unknown): Record<string, unknown> | undefined {
  return details && typeof details === 'object' && !Array.isArray(details)
    ? (details as Record<string, unknown>)
    : undefined;
}

export function detailNumber(details: unknown, keys: string[]): number | undefined {
  const record = detailRecord(details);
  if (!record) return undefined;
  for (const key of keys) {
    const value = record[key];
    if (typeof value === 'number' && Number.isFinite(value)) return value;
  }
  return undefined;
}

export function detailString(details: unknown, keys: string[]): string | undefined {
  const record = detailRecord(details);
  if (!record) return undefined;
  for (const key of keys) {
    const value = record[key];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return undefined;
}

/** Duration in ms from tool result details, when the harness reports it. */
export function resultDuration(result: ToolResult | undefined): number | undefined {
  if (!result) return undefined;
  const ms = detailNumber(result.details, ['durationMs', 'duration_ms', 'elapsedMs', 'elapsed_ms']);
  if (ms !== undefined) return ms;
  const seconds = detailNumber(result.details, ['duration', 'elapsed', 'seconds']);
  return seconds !== undefined ? seconds * 1000 : undefined;
}

export function exitCode(result: ToolResult | undefined): number | undefined {
  if (!result) return undefined;
  return detailNumber(result.details, ['exitCode', 'exit_code', 'code', 'status']);
}

export function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(1, Math.round(ms))}ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`;
  const minutes = Math.floor(ms / 60_000);
  const seconds = Math.round((ms % 60_000) / 1000);
  return `${minutes}m ${seconds}s`;
}

/** Flatten RpcContent[] to display text. */
export function contentText(content: RpcContent[] | undefined): string {
  if (!content) return '';
  return content
    .map((block) => {
      if (typeof block.text === 'string') return block.text;
      if (typeof block.thinking === 'string') return block.thinking;
      return '';
    })
    .filter((part) => part.length > 0)
    .join('\n');
}

/** Best-effort output text for a tool result or partial. */
export function resultText(result: ToolResult | undefined): string {
  if (!result) return '';
  const fromContent = contentText(result.content);
  if (fromContent) return fromContent;
  const output = detailString(result.details, ['output', 'stdout', 'text', 'result']);
  return output ?? '';
}

/** Compact single-line preview of a value for summaries. */
export function preview(value: string, max = 120): string {
  const flat = value.replace(/\s+/g, ' ').trim();
  return flat.length > max ? `${flat.slice(0, max - 1)}…` : flat;
}

/** Pretty-print args for the details disclosure (never shown by default).
 *  Pass `maxChars` only for display-only dumps, never for approval prompts. */
export function prettyJson(value: unknown, maxChars?: number): string {
  let text: string;
  try {
    text = JSON.stringify(value, null, 2) ?? '';
  } catch {
    text = String(value);
  }
  return maxChars !== undefined && text.length > maxChars ? `${text.slice(0, maxChars)}\n…` : text;
}

/** Extract http(s) URLs from tool args and result text. */
export function extractUrls(args: Record<string, unknown>, text: string): string[] {
  const urls = new Set<string>();
  const collect = (value: unknown) => {
    if (typeof value === 'string') {
      for (const match of value.match(/https?:\/\/[^\s"'<>)\]]+/g) ?? []) urls.add(match);
    } else if (Array.isArray(value)) {
      for (const entry of value) collect(entry);
    }
  };
  collect(args['url']);
  collect(args['urls']);
  collect(args['links']);
  collect(args['uri']);
  collect(text);
  return [...urls].slice(0, 20);
}

export function countLines(text: string): number {
  if (!text) return 0;
  let lines = 1;
  for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) lines++;
  return lines;
}

/** Heuristic match/file counts for search results when details lack them. */
export function searchCounts(text: string): { matches: number; files: number } | undefined {
  if (!text.trim()) return undefined;
  const lines = text.split('\n').filter((line) => line.trim().length > 0);
  const files = new Set<string>();
  let matches = 0;
  for (const line of lines) {
    const colon = line.match(/^(.+?):\d+[:.]/);
    if (colon) {
      files.add(colon[1]);
      matches++;
    } else if (line.includes(':')) {
      files.add(line.split(':')[0]);
    }
  }
  if (matches === 0) matches = lines.length;
  return { matches, files: files.size };
}

export function basename(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, '');
  const index = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'));
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}
