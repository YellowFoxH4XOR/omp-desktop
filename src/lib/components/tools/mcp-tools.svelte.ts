import { api } from '$lib/api';
import { detailString, resultText, type ToolItem } from './tool-utils';

/**
 * Configured MCP server names, loaded once. Direct MCP tools are named
 * `<server>_<tool>`; the name alone can't say which part is the server
 * until the result reports it, so the configured names fill that gap.
 */
export const mcpServers = $state<{ names: string[] }>({ names: [] });
let loading: Promise<void> | undefined;
export function loadMcpServers(): Promise<void> {
  loading ??= api.mcpOverview().then(
    overview => { mcpServers.names = overview.servers.map(server => server.name); },
    () => undefined,
  );
  return loading;
}

export interface McpCall {
  /** What happened, for the card's summary line. */
  kind: 'call' | 'script' | 'search' | 'describe' | 'connect' | 'list' | 'status' | 'instructions' | 'auth' | 'install' | 'other';
  server?: string;
  /** Tool name without the server prefix. */
  tool?: string;
  args?: Record<string, unknown>;
  query?: string;
  code?: string;
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}
function record(value: unknown): Record<string, unknown> | undefined {
  if (value && typeof value === 'object' && !Array.isArray(value)) return value as Record<string, unknown>;
  if (typeof value === 'string') {
    try { return record(JSON.parse(value)); } catch { return undefined; }
  }
  return undefined;
}

/** Split `server_tool` using the result's server or a configured name. */
function splitName(name: string, server?: string): { server?: string; tool: string } {
  const known = server ? [server] : [...mcpServers.names].sort((a, b) => b.length - a.length);
  for (const candidate of known) {
    for (const separator of ['_', '__', '-']) {
      if (name.startsWith(candidate + separator)) return { server: candidate, tool: name.slice(candidate.length + separator.length) };
    }
  }
  return { server, tool: name };
}

/** The MCP call this tool item represents, or null when it isn't one. */
export function mcpCall(item: ToolItem): McpCall | null {
  const name = item.toolName;
  const args = item.args ?? {};
  const details = item.result?.details;
  const resultServer = detailString(details, ['server']);
  if (name === 'mcpScript') return { kind: 'script', code: text(args.code) };
  if (name.startsWith('mcp__')) {
    const server = name.slice(5);
    const tool = text(args.tool);
    return tool ? { kind: 'call', server, tool: splitName(tool, server).tool, args: record(args.args) } : { kind: 'list', server };
  }
  if (name === 'mcp') {
    const tool = text(args.tool);
    if (tool) {
      const split = splitName(tool, resultServer ?? text(args.server));
      return { kind: 'call', server: split.server, tool: split.tool, args: record(args.args) };
    }
    const action = text(args.action);
    if (action?.startsWith('auth')) return { kind: 'auth', server: text(args.server) };
    if (action === 'install') return { kind: 'install', server: text(args.name) ?? text(args.url) };
    if (text(args.connect)) return { kind: 'connect', server: text(args.connect) };
    if (text(args.search)) return { kind: 'search', query: text(args.search) };
    if (text(args.describe)) return { kind: 'describe', ...splitName(text(args.describe)!) };
    if (text(args.instructions)) return { kind: 'instructions', server: text(args.instructions) };
    if (text(args.server)) return { kind: 'list', server: text(args.server) };
    return action ? { kind: 'other', tool: action } : { kind: 'status' };
  }
  const split = splitName(name, resultServer);
  if (split.server) return { kind: 'call', server: split.server, tool: detailString(details, ['tool']) ?? split.tool, args };
  return null;
}

/** `search_business_ideas` → "Search business ideas". */
export function humanize(tool: string): string {
  const words = tool.replace(/[_-]+/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2').trim().toLowerCase();
  return words ? words[0].toUpperCase() + words.slice(1) : tool;
}

/** Up to `max` short scalar arguments, for pills on the summary line. */
export function argPills(args: Record<string, unknown> | undefined, max = 3): Array<[string, string]> {
  if (!args) return [];
  const pills: Array<[string, string]> = [];
  for (const [key, value] of Object.entries(args)) {
    if (pills.length >= max) break;
    if (value === null || value === undefined || value === '') continue;
    if (typeof value === 'object') continue;
    const shown = String(value);
    pills.push([key.replace(/_/g, ' '), shown.length > 28 ? `${shown.slice(0, 27)}…` : shown]);
  }
  return pills;
}

const LIST_KEYS = ['results', 'items', 'data', 'ideas', 'records', 'rows', 'entries', 'matches', 'tools'];

/** A short outcome like "21 results" or the first line of the reply. */
export function resultSummary(item: ToolItem): string | undefined {
  const raw = resultText(item.result).trim();
  if (!raw) return undefined;
  let value: unknown;
  try { value = JSON.parse(raw); } catch { value = undefined; }
  const count = (list: unknown[]) => `${list.length} result${list.length === 1 ? '' : 's'}`;
  if (Array.isArray(value)) return count(value);
  if (value && typeof value === 'object') {
    for (const key of LIST_KEYS) {
      const list = (value as Record<string, unknown>)[key];
      if (Array.isArray(list)) return count(list);
    }
    const keys = Object.keys(value as object);
    return keys.length ? `{ ${keys.slice(0, 3).join(', ')}${keys.length > 3 ? ', …' : ''} }` : undefined;
  }
  const first = raw.split('\n').find(line => line.trim())?.trim() ?? '';
  return first.length > 60 ? `${first.slice(0, 59)}…` : first;
}

/** Pretty-print JSON results; anything else as-is. */
export function formatResult(raw: string): string {
  try { return JSON.stringify(JSON.parse(raw), null, 2); } catch { return raw; }
}
