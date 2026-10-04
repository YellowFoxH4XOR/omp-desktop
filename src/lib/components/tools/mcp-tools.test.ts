import { expect, test } from 'vitest';
import { mcpCall, mcpServers, mcpToolName, resultSummary } from './mcp-tools.svelte';
import type { ToolItem } from './tool-utils';

function toolItem(name: string, args: Record<string, unknown> = {}, details?: unknown): ToolItem {
  return {
    id: 'item', kind: 'tool', toolCallId: 'call', toolName: name, args, status: 'completed',
    ...(details !== undefined ? { result: { details } } : {}),
  };
}

test('mcpToolName prefers configured server names with dashes', () => {
  mcpServers.names = ['eureka-db', 'chrome-devtools', 'context7'];
  expect(mcpToolName('mcp__eureka_db__search_business_ideas')).toEqual({ server: 'eureka-db', tool: 'search_business_ideas' });
  expect(mcpToolName('mcp__chrome_devtools__click')).toEqual({ server: 'chrome-devtools', tool: 'click' });
});

test('mcpToolName splits unknown namespaces and rejects other names', () => {
  mcpServers.names = [];
  expect(mcpToolName('mcp__other_srv__do_thing')).toEqual({ server: 'other_srv', tool: 'do_thing' });
  expect(mcpToolName('mcp__other_srv__nested__part')).toEqual({ server: 'other_srv', tool: 'nested__part' });
  expect(mcpToolName('mcp__solo')).toBeNull();
  expect(mcpToolName('read')).toBeNull();
  expect(mcpToolName('eureka-db_get_saved_ideas')).toBeNull();
});

test('mcpCall reads built-in MCP tool kinds', () => {
  mcpServers.names = ['eureka-db'];
  expect(mcpCall(toolItem('codemode', { code: '// top ideas\ntext(1);' }))).toEqual({ kind: 'script', code: '// top ideas\ntext(1);' });
  expect(mcpCall(toolItem('tool_search', { query: 'business ideas' }))).toEqual({ kind: 'search', query: 'business ideas' });
  expect(mcpCall(toolItem('list_mcp_resources', {}))).toEqual({ kind: 'resources' });
  expect(mcpCall(toolItem('list_mcp_resource_templates', { server: 'eureka-db' }))).toEqual({ kind: 'resources', server: 'eureka-db', templates: true });
  expect(mcpCall(toolItem('read_mcp_resource', { server: 'eureka-db', uri: 'eureka://ideas/1' }))).toEqual({ kind: 'resource', server: 'eureka-db', uri: 'eureka://ideas/1' });
});

test('mcpCall prefers result details for server and tool', () => {
  mcpServers.names = ['eureka-db'];
  expect(mcpCall(toolItem('mcp__wrong_ns__raw_name', { limit: 1 }, { server: 'eureka-db', tool: 'get_saved_ideas' })))
    .toEqual({ kind: 'call', server: 'eureka-db', tool: 'get_saved_ideas', args: { limit: 1 } });
  expect(mcpCall(toolItem('mcp__eureka_db__search_business_ideas', { min_opportunity_score: 90 })))
    .toEqual({ kind: 'call', server: 'eureka-db', tool: 'search_business_ideas', args: { min_opportunity_score: 90 } });
});

test('adapter-era tool names are not MCP calls', () => {
  mcpServers.names = ['eureka-db'];
  expect(mcpCall(toolItem('mcp', { tool: 'eureka-db_search_business_ideas' }))).toBeNull();
  expect(mcpCall(toolItem('mcpScript', { code: 'emit(1);' }))).toBeNull();
  expect(mcpCall(toolItem('eureka-db_get_saved_ideas', {}))).toBeNull();
});

test('resultSummary reads a script tail line and JSON lists', () => {
  const script: ToolItem = {
    ...toolItem('codemode'),
    result: { content: [{ type: 'text', text: 'Script completed\nWall time 0.4 seconds\nOutput:\n[]' }] },
  };
  expect(resultSummary(script)).toBe('Script completed');
  const search: ToolItem = {
    ...toolItem('mcp__eureka_db__search_business_ideas'),
    result: { content: [{ type: 'text', text: JSON.stringify({ results: [1, 2] }) }] },
  };
  expect(resultSummary(search)).toBe('2 results');
});
