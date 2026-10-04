import { api } from '$lib/api';
import { detailString, resultText, type ToolItem } from './tool-utils';

/**
 * Configured MCP server names, loaded once. Built-in MCP tools are named
 * `mcp__<namespace>__<tool>`, where the namespace is the server name with
 * dashes turned into underscores; the configured names recover the real one.
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
  kind: 'call' | 'script' | 'search' | 'resources' | 'resource';
  server?: string;
  /** Tool name without the server namespace. */
  tool?: string;
  args?: Record<string, unknown>;
  query?: string;
  code?: string;
  uri?: string;
  templates?: boolean;
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined;
}

/** Split a built-in `mcp__<namespace>__<tool>` tool name into server and tool.
 *  Prefers a configured server whose namespace prefixes the name. */
export function mcpToolName(name: string): { server: string; tool: string } | null {
  if (!name.startsWith('mcp__')) return null;
  const rest = name.slice(5);
  const known = [...mcpServers.names].sort((a, b) => b.length - a.length);
  for (const server of known) {
    const namespace = server.replace(/-/g, '_');
    if (rest.startsWith(`${namespace}__`) && rest.length > namespace.length + 2) {
      return { server, tool: rest.slice(namespace.length + 2) };
    }
  }
  const separator = rest.indexOf('__');
  if (separator <= 0 || separator + 2 >= rest.length) return null;
  return { server: rest.slice(0, separator), tool: rest.slice(separator + 2) };
}

/** The built-in MCP call this tool item represents, or null when it isn't one. */
export function mcpCall(item: ToolItem): McpCall | null {
  const name = item.toolName;
  const args = item.args ?? {};
  switch (name) {
    case 'codemode':
      return { kind: 'script', code: text(args.code) };
    case 'tool_search':
      return { kind: 'search', query: text(args.query) };
    case 'list_mcp_resources':
      return { kind: 'resources', server: text(args.server) };
    case 'list_mcp_resource_templates':
      return { kind: 'resources', server: text(args.server), templates: true };
    case 'read_mcp_resource':
      return { kind: 'resource', server: text(args.server), uri: text(args.uri) };
  }
  if (!name.startsWith('mcp__')) return null;
  const details = item.result?.details;
  const split = mcpToolName(name);
  return {
    kind: 'call',
    server: detailString(details, ['server']) ?? split?.server,
    tool: detailString(details, ['tool']) ?? split?.tool,
    args,
  };
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
