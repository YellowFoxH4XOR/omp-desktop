/*
 * SessionModel — normalized, reactive conversation state for one thread.
 *
 * Flattens get_messages history into ConversationItem rows and reduces live
 * RPC frames (OMP + Pi) into the same SessionView. UI components never see
 * raw harness frames. Backend strips assistantMessageEvent.partial and
 * message_update.message before forwarding; this reducer tolerates both the
 * stripped OMP shape and Pi's delta-only shape.
 */

import type {
	AgentInfo,
	ConversationItem,
	ContextUsage,
	ModelInfo,
	RpcContent,
	RpcMessage,
	SessionSnapshot,
	SessionView,
	ThreadStatus,
	ToolResult,
	UiRequest,
	Usage,
} from './types';

type ToolItem = Extract<ConversationItem, { kind: 'tool' }>;
type AgentStatus = AgentInfo['status'];
type TextItem = Extract<ConversationItem, { text: string }>;

interface LiveBlock {
	id: string;
	kind: 'text' | 'thinking' | 'tool';
	item: ConversationItem;
	rawArgs: string;
}

interface LiveState {
	blocks: Map<number, LiveBlock>;
	order: number[];
	timestamp?: number;
}

interface BashLive {
	item: ConversationItem;
	output: string;
}

let itemSeq = 0;
function nid(): string {
	return `ci${itemSeq++}`;
}

/* ---------------- guards / coercion ---------------- */

function isRec(v: unknown): v is Record<string, unknown> {
	return typeof v === 'object' && v !== null && !Array.isArray(v);
}
function str(v: unknown): string | undefined {
	return typeof v === 'string' ? v : undefined;
}
function num(v: unknown): number | undefined {
	return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}
function arr(v: unknown): unknown[] | undefined {
	return Array.isArray(v) ? v : undefined;
}
function clean<T extends Record<string, unknown>>(o: T): T {
	const out = { ...o };
	for (const k of Object.keys(out)) if (out[k] === undefined) delete out[k];
	return out;
}

/* ---------------- message helpers ---------------- */

function contentToText(content: unknown): string {
	if (typeof content === 'string') return content;
	if (!Array.isArray(content)) return '';
	const parts: string[] = [];
	for (const b of content) {
		if (isRec(b) && b.type === 'text' && typeof b.text === 'string') parts.push(b.text);
	}
	return parts.join('\n');
}

function assistantBlocks(msg: RpcMessage): RpcContent[] {
	const c = msg.content;
	if (typeof c === 'string') return c ? [{ type: 'text', text: c }] : [];
	if (!Array.isArray(c)) return [];
	return c.filter((b): b is RpcContent => isRec(b) && typeof b.type === 'string');
}

function normalizeContent(content: unknown): RpcContent[] | undefined {
	if (typeof content === 'string') return content ? [{ type: 'text', text: content }] : undefined;
	if (!Array.isArray(content)) return undefined;
	const out: RpcContent[] = [];
	for (const b of content) if (isRec(b) && typeof b.type === 'string') out.push(b as RpcContent);
	return out.length ? out : undefined;
}

function normalizeResult(v: unknown): ToolResult | undefined {
	if (!isRec(v)) return undefined;
	const r: ToolResult = {};
	const c = normalizeContent(v.content);
	if (c) r.content = c;
	if (v.details !== undefined) r.details = v.details;
	if (typeof v.isError === 'boolean') r.isError = v.isError;
	return r;
}

function fingerprint(m: RpcMessage): string {
	const t = m.timestamp ?? 0;
	if (m.role === 'toolResult') return `tr|${m.toolCallId}|${t}`;
	if (m.role === 'bashExecution') return `bash|${m.command}|${m.exitCode}|${t}`;
	const c = m.content;
	if (typeof c === 'string') return `${m.role}|${t}|s${c.length}`;
	if (Array.isArray(c)) {
		let sig = '';
		for (const b of c) {
			if (!isRec(b)) continue;
			sig +=
				b.type === 'toolCall'
					? `T${str(b.id)}`
					: `${b.type}:${(str(b.text) ?? str(b.thinking) ?? '').length};`;
		}
		return `${m.role}|${t}|${c.length}|${sig}`;
	}
	return `${m.role}|${t}|${str(m.customType) ?? ''}`;
}

function mapAgentStatus(s: string | undefined): AgentStatus | undefined {
	switch (s) {
		case 'started':
		case 'running':
		case 'in_progress':
			return 'running';
		case 'pending':
			return 'pending';
		case 'waiting':
		case 'idle':
			return 'waiting';
		case 'completed':
		case 'complete':
		case 'done':
			return 'completed';
		case 'failed':
		case 'error':
			return 'failed';
		case 'aborted':
		case 'cancelled':
		case 'killed':
			return 'aborted';
		case 'parked':
			return 'parked';
		default:
			return undefined;
	}
}

function mapLevel(s: string | undefined): string {
	if (s === 'warning') return 'warn';
	if (s === 'success') return 'info';
	return s ?? 'info';
}

function normalizeModel(v: unknown): ModelInfo | undefined {
	if (!isRec(v)) return undefined;
	const provider = str(v.provider);
	const id = str(v.id) ?? str(v.modelId);
	if (!provider || !id) return undefined;
	return {
		provider,
		id,
		name: str(v.name) ?? id,
		contextWindow: num(v.contextWindow),
		reasoning: typeof v.reasoning === 'boolean' ? v.reasoning : undefined,
	};
}

function parseUsage(u: Record<string, unknown>): Usage | undefined {
	const tokens = isRec(u.tokens) ? u.tokens : u;
	const t = {
		input: num(tokens.input) ?? 0,
		output: num(tokens.output) ?? 0,
		cacheRead: num(tokens.cacheRead) ?? 0,
		cacheWrite: num(tokens.cacheWrite) ?? 0,
		total: num(tokens.total) ?? num(tokens.totalTokens) ?? 0,
	};
	const cost = isRec(u.cost) ? num(u.cost.total) ?? 0 : num(u.cost) ?? 0;
	if (!t.input && !t.output && !t.total && !cost) return undefined;
	return { tokens: t, cost };
}

function advisorLevel(details: unknown): string | undefined {
	if (!isRec(details)) return undefined;
	const notes = arr(details.notes);
	const first = notes?.[0];
	if (isRec(first)) return str(first.severity);
	return undefined;
}

function dottedParent(id: string): string | undefined {
	const i = id.lastIndexOf('.');
	return i > 0 ? id.slice(0, i) : undefined;
}

/** Narrow an untrusted frame field to RpcMessage (requires a string role). */
function asMsg(v: unknown): RpcMessage | null {
	return isRec(v) && typeof v.role === 'string' ? (v as unknown as RpcMessage) : null;
}

/* ---------------- item sink (shared by history + live) ---------------- */

class ItemSink {
	constructor(
		readonly items: ConversationItem[],
		readonly toolIndex: Map<string, ToolItem>,
	) {}

	push(item: ConversationItem): ConversationItem {
		this.items.push(item);
		return this.items[this.items.length - 1];
	}

	pushUser(msg: RpcMessage): void {
		const text = contentToText(msg.content);
		if (!text) return;
		this.push({ id: nid(), kind: 'user', text, timestamp: msg.timestamp });
	}

	/** Push one assistant content block. Returns the stored item, an existing
	 *  deduped tool item, or null for empty/unsupported blocks. */
	pushBlock(block: RpcContent, timestamp: number | undefined, streaming: boolean): ConversationItem | null {
		if (block.type === 'toolCall') {
			const toolCallId = str(block.id) ?? '';
			const existing = toolCallId ? this.toolIndex.get(toolCallId) : undefined;
			if (existing) {
				existing.toolName = str(block.name) ?? existing.toolName;
				if (isRec(block.arguments)) existing.args = block.arguments;
				return existing;
			}
			const raw: ToolItem = {
				id: nid(),
				kind: 'tool',
				toolCallId,
				toolName: str(block.name) ?? 'tool',
				args: isRec(block.arguments) ? block.arguments : {},
				status: 'queued',
				timestamp,
			};
			const stored = this.push(raw) as ToolItem;
			if (toolCallId) this.toolIndex.set(toolCallId, stored);
			return stored;
		}
		const made = makeTextItem(block, timestamp, streaming);
		if (!made) return null;
		return this.push(made);
	}

	pushAssistant(msg: RpcMessage, streaming = false): ConversationItem[] {
		const out: ConversationItem[] = [];
		for (const b of assistantBlocks(msg)) {
			const it = this.pushBlock(b, msg.timestamp, streaming);
			if (it) out.push(it);
		}
		return out;
	}

	pairToolResult(msg: RpcMessage): void {
		const toolCallId = str(msg.toolCallId);
		const result: ToolResult = {};
		const content = normalizeContent(msg.content);
		if (content) result.content = content;
		if (msg.details !== undefined) result.details = msg.details;
		if (typeof msg.isError === 'boolean') result.isError = msg.isError;
		const status = msg.isError ? 'failed' : 'completed';
		const existing = toolCallId ? this.toolIndex.get(toolCallId) : undefined;
		if (existing) {
			existing.result = result;
			existing.partial = undefined;
			existing.status = status;
			return;
		}
		if (!toolCallId) return;
		const stored = this.push({
			id: nid(),
			kind: 'tool',
			toolCallId,
			toolName: str(msg.toolName) ?? 'tool',
			args: {},
			status,
			result,
			timestamp: msg.timestamp,
		}) as ToolItem;
		this.toolIndex.set(toolCallId, stored);
	}

	pushBash(msg: RpcMessage): ConversationItem {
		const m = msg as unknown as Record<string, unknown>;
		const details: Record<string, unknown> = { ...(isRec(msg.details) ? msg.details : {}) };
		if (msg.command !== undefined) details.command = msg.command;
		if (msg.output !== undefined) details.output = msg.output;
		if (msg.exitCode !== undefined) details.exitCode = msg.exitCode;
		if (m.cancelled !== undefined) details.cancelled = m.cancelled;
		if (m.truncated !== undefined) details.truncated = m.truncated;
		if (m.fullOutputPath !== undefined) details.fullOutputPath = m.fullOutputPath;
		return this.push({
			id: nid(),
			kind: 'custom',
			customType: 'bashExecution',
			text: str(msg.command) ?? '',
			details,
			timestamp: msg.timestamp,
		});
	}

	pushCustom(msg: RpcMessage): ConversationItem | null {
		if (msg.display === false) return null;
		const text = contentToText(msg.content);
		const customType = str(msg.customType);
		if (customType === 'advisor') {
			return this.push({
				id: nid(),
				kind: 'advisor',
				text,
				customType,
				details: msg.details,
				level: advisorLevel(msg.details),
				timestamp: msg.timestamp,
			});
		}
		return this.push({
			id: nid(),
			kind: 'custom',
			text,
			customType,
			details: msg.details,
			timestamp: msg.timestamp,
		});
	}

	pushCompaction(msg: RpcMessage): void {
		const m = msg as unknown as Record<string, unknown>;
		const text = str(m.summary) || contentToText(msg.content) || 'Context compacted';
		const details =
			msg.details !== undefined
				? msg.details
				: clean({
						shortSummary: str(m.shortSummary),
						tokensBefore: num(m.tokensBefore),
						tokensAfter: num(m.tokensAfter),
						method: str(m.method),
					});
		this.push({ id: nid(), kind: 'custom', customType: 'compaction', text, details, timestamp: msg.timestamp });
	}

	pushBranchSummary(msg: RpcMessage): void {
		const m = msg as unknown as Record<string, unknown>;
		const text = str(m.summary) || contentToText(msg.content) || 'Branch summary';
		const details = msg.details !== undefined ? msg.details : clean({ fromId: str(m.fromId) });
		this.push({ id: nid(), kind: 'custom', customType: 'branchSummary', text, details, timestamp: msg.timestamp });
	}

	ingest(msg: RpcMessage): void {
		switch (msg.role) {
			case 'user':
				this.pushUser(msg);
				return;
			case 'assistant':
				this.pushAssistant(msg);
				return;
			case 'toolResult':
				this.pairToolResult(msg);
				return;
			case 'bashExecution':
				this.pushBash(msg);
				return;
			case 'custom':
			case 'hookMessage':
				this.pushCustom(msg);
				return;
			case 'compactionSummary':
				this.pushCompaction(msg);
				return;
			case 'branchSummary':
				this.pushBranchSummary(msg);
				return;
			case 'developer':
			case 'system':
				return;
			default: {
				const text = contentToText(msg.content);
				if (text)
					this.push({
						id: nid(),
						kind: 'custom',
						text,
						customType: msg.role,
						details: msg.details,
						timestamp: msg.timestamp,
					});
			}
		}
	}
}

function makeTextItem(
	block: RpcContent,
	timestamp: number | undefined,
	streaming: boolean,
): ConversationItem | null {
	if (block.type === 'text') {
		const text = str(block.text) ?? '';
		if (!text && !streaming) return null;
		return { id: nid(), kind: 'text', text, streaming, timestamp };
	}
	if (block.type === 'thinking') {
		const text = str(block.thinking) ?? '';
		if (!text && !streaming) return null;
		return { id: nid(), kind: 'thinking', text, streaming, timestamp };
	}
	return null;
}

/** Flatten get_messages history into ordered conversation rows. Tool results
 *  are paired back onto their toolCall item by toolCallId. */
export function messagesToItems(messages: RpcMessage[]): ConversationItem[] {
	const items: ConversationItem[] = [];
	const sink = new ItemSink(items, new Map());
	for (const m of messages) if (isRec(m)) sink.ingest(m);
	return items;
}

/* ---------------- session model ---------------- */

export class SessionModel {
	/** Public reactive view — mutate fields directly (model/effort/usage). */
	view: SessionView;

	#sink: ItemSink;
	#toolIndex = new Map<string, ToolItem>();
	#live: LiveState | null = null;
	#staleLiveIds = new Set<string>();
	#seen = new Set<string>();
	#seenQueue: string[] = [];
	#bashLive = new Map<string, BashLive>();
	#runActive = false;
	#idleStatus: ThreadStatus = 'idle';
	#failed = false;

	constructor(snapshot: SessionSnapshot) {
		const msgs = Array.isArray(snapshot.messages) ? snapshot.messages : [];
		const streaming = snapshot.state?.isStreaming === true;
		const last = msgs[msgs.length - 1];
		const tail = streaming && last?.role === 'assistant' && last.stopReason === 'pending' ? last : undefined;
		const history = tail ? msgs.slice(0, -1) : msgs;

		this.#runActive = streaming;
		const ts = snapshot.thread?.status;
		this.#idleStatus = ts === 'completed' || ts === 'failed' ? ts : 'idle';

		this.view = $state<SessionView>({
			threadId: snapshot.thread?.id ?? '',
			items: [],
			agents: Array.isArray(snapshot.agents) ? snapshot.agents.map((a) => ({ ...a })) : [],
			pendingRequests: [],
			status: streaming ? 'active' : this.#idleStatus,
			capabilities: snapshot.capabilities,
			model: snapshot.state?.model,
			effort: snapshot.state?.thinkingLevel,
			contextUsage: snapshot.state?.contextUsage,
			sessionFile: snapshot.state?.sessionFile ?? snapshot.thread?.sessionFile,
			models: Array.isArray(snapshot.models) ? snapshot.models : [],
			levels: Array.isArray(snapshot.levels) ? snapshot.levels : [],
			commands: [],
		});

		this.#sink = new ItemSink(this.view.items, this.#toolIndex);
		for (const m of history) {
			if (!isRec(m)) continue;
			this.#sink.ingest(m);
			this.#markSeen(fingerprint(m));
		}
		if (tail) this.#adoptTail(tail);
	}

	/* ---- public API ---- */

	apply(frame: Record<string, unknown>): void {
		if (!isRec(frame)) return;
		if (frame.type === 'rpc' && isRec(frame.frame)) {
			this.apply(frame.frame);
			return;
		}
		const type = str(frame.type);
		if (!type) return;
		try {
			this.#dispatch(type, frame);
		} catch {
			console.error('[SessionModel] frame handling failed:', type);
		}
	}

	setError(error: string): void {
		this.view.error = error;
		this.#failed = true;
		this.#runActive = false;
		this.#finalizeLive();
		for (const it of this.#toolIndex.values()) {
			if (it.status === 'running' || it.status === 'queued') it.status = 'cancelled';
		}
		this.view.pendingRequests = [];
		this.view.status = 'disconnected';
	}

	/** Remove a pending UI request after the shell delivered the response. */
	dismissRequest(requestId: string): void {
		this.#removeRequest(requestId);
	}

	/** Reconnect to the same harness session without discarding visible work.
	 * The journal may not contain the interrupted streaming tail, and OMP's
	 * get_messages can collapse older context. Keep those rows, reconcile
	 * persisted completions, then let subsequent RPC frames append normally. */
	reconnect(snapshot: SessionSnapshot): void {
		const restored = new SessionModel(snapshot);
		const rows = this.view.items;
		const keyOf = (item: ConversationItem): string => {
			if (item.kind === 'tool') return `tool:${item.toolCallId || item.id}`;
			return `${item.kind}:${item.timestamp ?? ''}:${item.text}:${
				item.kind === 'custom' || item.kind === 'notice' || item.kind === 'advisor'
					? (item.customType ?? '') : ''}`;
		};
		const existing = new Map<string, ConversationItem[]>();
		const unfinished = new Map<string, ConversationItem[]>();
		for (const row of rows) {
			const key = keyOf(row);
			const bucket = existing.get(key) ?? [];
			bucket.push(row);
			existing.set(key, bucket);
			if ((row.kind === 'text' || row.kind === 'thinking') && row.text) {
				const blockKey = `${row.kind}:${row.timestamp ?? ''}`;
				const blocks = unfinished.get(blockKey) ?? [];
				blocks.push(row);
				unfinished.set(blockKey, blocks);
			}
		}
		const appended: ConversationItem[] = [];
		for (const row of restored.view.items) {
			const matches = existing.get(keyOf(row));
			if (matches?.length) {
				const prior = matches.shift();
				if (prior?.kind === 'tool' && row.kind === 'tool') {
					prior.status = row.status;
					prior.result = row.result ?? prior.result;
					prior.args = row.args;
				}
				continue;
			}
			if (row.kind === 'text' || row.kind === 'thinking') {
				const blockKey = `${row.kind}:${row.timestamp ?? ''}`;
				const blocks = unfinished.get(blockKey);
				const partial = blocks?.find(candidate =>
					(candidate.kind === 'text' || candidate.kind === 'thinking') &&
					row.text.startsWith(candidate.text) && candidate.text.length < row.text.length);
				if (partial && (partial.kind === 'text' || partial.kind === 'thinking')) {
					partial.text = row.text;
					partial.streaming = false;
					blocks?.splice(blocks.indexOf(partial), 1);
					continue;
				}
			}
			appended.push(row);
		}
		for (const row of rows) {
			if (row.kind === 'text' || row.kind === 'thinking') row.streaming = false;
			if (row.kind === 'tool' && (row.status === 'running' || row.status === 'queued')) row.status = 'cancelled';
		}
		this.view.items = [...rows, ...appended];
		this.view.agents = restored.view.agents.length ? restored.view.agents : this.view.agents;
		this.view.pendingRequests = [];
		this.view.status = restored.view.status;
		this.view.error = undefined;
		this.view.capabilities = restored.view.capabilities;
		this.view.model = restored.view.model;
		this.view.effort = restored.view.effort;
		this.view.contextUsage = restored.view.contextUsage;
		this.view.sessionFile = restored.view.sessionFile;
		this.view.models = restored.view.models;
		this.view.levels = restored.view.levels;
		this.#runActive = restored.#runActive;
		this.#idleStatus = restored.#idleStatus;
		this.#failed = false;
		this.#live = restored.#live;
		this.#staleLiveIds = restored.#staleLiveIds;
		this.#seen = restored.#seen;
		this.#seenQueue = restored.#seenQueue;
		this.#bashLive = restored.#bashLive;
		this.#toolIndex = new Map();
		for (const row of this.view.items) if (row.kind === 'tool' && row.toolCallId) this.#toolIndex.set(row.toolCallId, row);
		this.#sink = new ItemSink(this.view.items, this.#toolIndex);
	}

	/* ---- frame dispatch ---- */

	#dispatch(type: string, frame: Record<string, unknown>): void {
		switch (type) {
			case 'agent_start':
				this.#runActive = true;
				this.#failed = false;
				this.#idleStatus = 'idle';
				this.view.error = undefined;
				this.#refreshStatus();
				return;
			case 'agent_end': {
				const continuing = frame.isTerminal === false || frame.willRetry === true;
				this.#runActive = continuing;
				if (!continuing && this.#idleStatus !== 'failed') this.#idleStatus = 'completed';
				this.#finalizeLive();
				this.#refreshStatus();
				return;
			}
			case 'agent_settled':
			case 'run_end':
				this.#runActive = false;
				if (this.#idleStatus !== 'failed') this.#idleStatus = 'completed';
				this.#finalizeLive();
				this.#refreshStatus();
				if (isRec(frame.error)) this.#notice('error', str(frame.error.message) ?? 'Run failed');
				return;
			case 'operation_abort':
				this.#runActive = false;
				this.#finalizeLive();
				this.#refreshStatus();
				return;
			case 'turn_end': {
				const tm = asMsg(frame.message);
				if (tm) this.#ingestLive(tm);
				const trs = arr(frame.toolResults);
				if (trs) for (const t of trs) { const tr = asMsg(t); if (tr) this.#ingestLive(tr); }
				return;
			}
			case 'message_start': {
				const m = asMsg(frame.message);
				if (m?.role === 'assistant') {
					// Some providers put the completed text in message_start and
					// then stream the same text again as deltas. Only the deltas
					// populate live rows; message_end reconciles authoritatively.
					if (this.#live) this.#supersedeLive();
					this.#ensureLive().timestamp = m.timestamp;
				}
				return;
			}
			case 'message_update':
				this.#onMessageUpdate(frame);
				return;
			case 'message_end':
			case 'message': {
				// Nested under `message`, or the frame itself is the message.
				const em = asMsg(frame.message) ?? asMsg(frame);
				if (em) this.#ingestLive(em);
				return;
			}
			case 'custom_message': {
				const m = asMsg(frame.message) ?? this.#customFrameMessage(frame);
				if (m) this.#ingestLive(m);
				return;
			}
			case 'tool_execution_start':
			case 'tool_start':
				this.#onToolStart(frame);
				return;
			case 'tool_execution_update':
			case 'tool_stream_update':
			case 'tool_update':
				this.#onToolUpdate(frame);
				return;
			case 'tool_execution_end':
			case 'tool_end':
				this.#onToolEnd(frame);
				return;
			case 'extension_ui_request':
				this.#onUiRequest(frame);
				return;
			case 'subagent_lifecycle':
				this.#onSubagentLifecycle(frame);
				return;
			case 'subagent_progress':
				this.#onSubagentProgress(frame);
				return;
			case 'available_commands_update': {
				const cmds = arr(frame.commands);
				if (!cmds) return;
				this.view.commands = cmds
					.filter(isRec)
					.map((c) => ({ name: str(c.name) ?? '', description: str(c.description) }))
					.filter((c) => c.name.length > 0);
				return;
			}
			case 'command_output':
				this.#notice('info', str(frame.text) ?? '');
				return;
			case 'notice':
				this.#notice(mapLevel(str(frame.level)), str(frame.message) ?? str(frame.text) ?? '', frame.source);
				return;
			case 'auto_retry_start':
			case 'retry_start':
			case 'retry_scheduled': {
				const a = num(frame.attempt);
				const m = num(frame.maxAttempts);
				const err = str(frame.errorMessage);
				this.#notice(
					'warn',
					`Retrying request${a !== undefined ? ` (attempt ${a}${m !== undefined ? `/${m}` : ''})` : ''}${err ? `: ${err}` : ''}`,
				);
				return;
			}
			case 'auto_retry_end':
			case 'retry_end': {
				if (frame.success === true) {
					this.#notice('info', 'Retry succeeded');
				} else if (frame.success === false) {
					const fe = str(frame.finalError) ?? str(frame.errorMessage);
					this.#notice('error', `Request failed${fe ? `: ${fe}` : ''}`);
				}
				return;
			}
			case 'auto_compaction_start':
			case 'compaction_start':
				this.#notice('info', 'Compacting context…', clean({ reason: str(frame.reason), action: str(frame.action) }));
				return;
			case 'auto_compaction_end': {
				const err = str(frame.errorMessage);
				if (err) this.#notice('error', `Compaction failed: ${err}`);
				else if (frame.aborted === true) this.#notice('warn', 'Compaction aborted');
				else if (frame.skipped === true) return;
				else this.#notice('info', `Context compacted${frame.willRetry === true ? ' — retrying' : ''}`);
				return;
			}
			case 'compaction_end': {
				const st = str(frame.status);
				if (st === 'completed') this.#notice('info', 'Context compacted');
				else if (st && st !== 'declined') this.#notice('warn', `Compaction ${st}`);
				return;
			}
			case 'retry_fallback_applied': {
				const to = str(frame.to);
				this.#notice('info', `Model fallback${to ? `: switched to ${to}` : ' applied'}`, clean({ from: str(frame.from), reason: str(frame.reason) }));
				return;
			}
			case 'retry_fallback_succeeded':
				this.#notice('info', `Fallback succeeded${str(frame.model) ? `: ${str(frame.model)}` : ''}`);
				return;
			case 'thinking_level_changed':
				this.view.effort = str(frame.thinkingLevel) ?? str(frame.resolved) ?? this.view.effort;
				return;
			case 'config_update':
				this.#onConfigUpdate(frame);
				return;
			case 'goal_updated':
				this.#notice('info', str(frame.goal) ?? str(frame.state) ?? 'Goal updated');
				return;
			case 'response': {
				if (frame.success !== false) return;
				const command = str(frame.command) ?? 'command';
				this.#notice('error', `${command}: ${this.#errorText(frame) ?? 'The harness could not continue.'}`);
				if (['prompt', 'abort_and_prompt', 'steer', 'follow_up'].includes(command)) {
					this.#runActive = false;
					this.#idleStatus = 'failed';
					this.#finalizeLive();
					this.#refreshStatus();
				}
				return;
			}
			case 'extension_error':
			case 'rpc_frame_error':
			case 'handler_error':
			case 'fault':
			case 'api_error':
			case 'authentication_error':
			case 'rate_limit_error':
			case 'upstream_error':
			case 'invalid_request_error':
			case 'error':
				this.#notice('error', this.#errorText(frame) ?? 'Harness error');
				return;
			case 'request_aborted':
				this.#notice('info', 'Request aborted');
				return;
			case 'warning':
				this.#notice('warn', str(frame.message) ?? str(frame.text) ?? 'Warning');
				return;
			case 'usage':
				this.#onUsageEvent(frame);
				return;
			case 'bash_execution_update':
				this.#onBashUpdate(frame);
				return;
			case 'entry_added':
				this.#onEntryAdded(frame);
				return;
			case 'compaction': {
				const m = {
					role: 'compactionSummary',
					content: str(frame.summary),
					details: frame.details,
					timestamp: num(frame.timestamp),
				} as RpcMessage;
				this.#ingestLive(m);
				return;
			}
			case 'branch_summary': {
				const m = {
					role: 'branchSummary',
					content: str(frame.summary),
					timestamp: num(frame.timestamp),
				} as RpcMessage;
				this.#ingestLive(m);
				return;
			}
			case 'exited': {
				if (frame.expected === true) {
					this.#runActive = false;
					this.#failed = true;
					this.#finalizeLive();
					this.view.pendingRequests = [];
					this.view.status = 'disconnected';
				} else {
					this.setError(str(frame.stderr) ?? 'Harness process exited unexpectedly');
				}
				return;
			}
			default:
				// turn_start, queue_update, subagent_event, model_changed,
				// session_info_update, status_changed, pong, ready,
				// todo_reminder, ttsr_triggered, navigation_*, lane_created,
				// value_update, run_*, session_*, host_tool_*, irc_message —
				// safe to ignore.
				return;
		}
	}

	/* ---- live message ingestion ---- */

	#markSeen(fp: string): void {
		if (this.#seen.has(fp)) return;
		this.#seen.add(fp);
		this.#seenQueue.push(fp);
		if (this.#seenQueue.length > 2000) {
			const old = this.#seenQueue.splice(0, this.#seenQueue.length - 2000);
			for (const f of old) this.#seen.delete(f);
		}
	}

	#ingestLive(msg: RpcMessage): void {
		const fp = fingerprint(msg);
		if (this.#seen.has(fp)) return;
		this.#markSeen(fp);
		if (msg.role === 'assistant') {
			this.#reconcileAssistant(msg);
			return;
		}
		if (msg.role === 'bashExecution') {
			this.#ingestBash(msg);
			return;
		}
		this.#sink.ingest(msg);
	}

	#customFrameMessage(frame: Record<string, unknown>): RpcMessage | null {
		const customType = str(frame.customType);
		if (!customType) return null;
		return {
			role: 'custom',
			customType,
			content: frame.content as RpcMessage['content'],
			display: frame.display === false ? false : undefined,
			details: frame.details,
			timestamp: num(frame.timestamp),
		};
	}

	/* ---- streaming assistant message ---- */

	#ensureLive(): LiveState {
		if (!this.#live) this.#live = { blocks: new Map(), order: [] };
		return this.#live;
	}

	#adoptTail(msg: RpcMessage): void {
		const live = this.#ensureLive();
		live.timestamp = msg.timestamp;
		this.#seedLive(live, msg);
	}

	#seedLive(live: LiveState, msg: RpcMessage): void {
		const blocks = assistantBlocks(msg);
		blocks.forEach((b, i) => {
			if (live.blocks.has(i)) return;
			const item = this.#sink.pushBlock(b, msg.timestamp, true);
			if (!item) return;
			const kind = b.type === 'toolCall' ? 'tool' : b.type === 'thinking' ? 'thinking' : 'text';
			live.blocks.set(i, { id: item.id, kind, item, rawArgs: '' });
			live.order.push(i);
		});
	}

	#contentIndex(live: LiveState, ev: Record<string, unknown>): number {
		const i = num(ev.contentIndex);
		if (i !== undefined) return i;
		return live.order.length ? live.order[live.order.length - 1] : 0;
	}

	#createBlock(live: LiveState, idx: number, kind: LiveBlock['kind'], ev: Record<string, unknown>): LiveBlock {
		let item: ConversationItem;
		if (kind === 'tool') {
			const raw: ToolItem = {
				id: nid(),
				kind: 'tool',
				toolCallId: str(ev.id) ?? str(ev.toolCallId) ?? '',
				toolName: str(ev.toolName) ?? str(ev.name) ?? 'tool',
				args: {},
				status: 'queued',
				timestamp: live.timestamp,
			};
			this.view.items.push(raw);
			item = this.view.items[this.view.items.length - 1];
			const tcid = (item as ToolItem).toolCallId;
			if (tcid) this.#toolIndex.set(tcid, item as ToolItem);
		} else {
			this.view.items.push({ id: nid(), kind, text: '', streaming: true, timestamp: live.timestamp });
			item = this.view.items[this.view.items.length - 1];
		}
		const b: LiveBlock = { id: item.id, kind, item, rawArgs: '' };
		live.blocks.set(idx, b);
		live.order.push(idx);
		return b;
	}

	#blockFor(live: LiveState, ev: Record<string, unknown>, kind: LiveBlock['kind']): LiveBlock {
		const idx = this.#contentIndex(live, ev);
		return live.blocks.get(idx) ?? this.#createBlock(live, idx, kind, ev);
	}

	#settleLive(): void {
		const live = this.#live;
		if (!live) return;
		for (const b of live.blocks.values()) {
			if (b.kind !== 'tool') (b.item as { streaming?: boolean }).streaming = false;
		}
	}

	/** A new assistant message started while a previous one was never
	 *  reconciled: mark its rows stale so the next reconcile removes them. */
	#supersedeLive(): void {
		const live = this.#live;
		if (live) for (const b of live.blocks.values()) this.#staleLiveIds.add(b.id);
		this.#settleLive();
		this.#live = null;
	}

	#finalizeLive(): void {
		this.#settleLive();
		this.#live = null;
	}

	#onMessageUpdate(frame: Record<string, unknown>): void {
		const ev = frame.assistantMessageEvent;
		if (!isRec(ev)) {
			// Unstripped OMP frame carrying the full message — reconcile directly.
			const um = asMsg(frame.message);
			if (um) this.#ingestLive(um);
			return;
		}
		const et = str(ev.type);
		const live = this.#ensureLive();
		switch (et) {
			case 'start':
				return;
			case 'text_start':
			case 'thinking_start':
				this.#blockFor(live, ev, et === 'text_start' ? 'text' : 'thinking');
				return;
			case 'text_delta':
			case 'thinking_delta': {
				const b = this.#blockFor(live, ev, et === 'text_delta' ? 'text' : 'thinking');
				(b.item as TextItem).text += str(ev.delta) ?? '';
				return;
			}
			case 'text_end':
			case 'thinking_end': {
				const b = this.#blockFor(live, ev, et === 'text_end' ? 'text' : 'thinking');
				const content = str(ev.content);
				if (content !== undefined) (b.item as TextItem).text = content;
				(b.item as { streaming?: boolean }).streaming = false;
				return;
			}
			case 'toolcall_start':
				this.#blockFor(live, ev, 'tool');
				return;
			case 'toolcall_delta': {
				const b = this.#blockFor(live, ev, 'tool');
				b.rawArgs += str(ev.delta) ?? '';
				return;
			}
			case 'toolcall_end': {
				const b = this.#blockFor(live, ev, 'tool');
				const item = b.item as ToolItem;
				const tc = isRec(ev.toolCall) ? ev.toolCall : undefined;
				const id = str(tc?.id) ?? str(ev.id) ?? str(ev.toolCallId);
				if (id && id !== item.toolCallId) {
					if (item.toolCallId) this.#toolIndex.delete(item.toolCallId);
					item.toolCallId = id;
					this.#toolIndex.set(id, item);
				}
				item.toolName = str(tc?.name) ?? str(ev.toolName) ?? str(ev.name) ?? item.toolName;
				const args = isRec(tc?.arguments) ? tc?.arguments : isRec(ev.arguments) ? ev.arguments : undefined;
				if (args) item.args = args;
				return;
			}
			case 'done':
				// Usage intentionally not accumulated here: the following
				// message_end carries the same usage object.
				this.#settleLive();
				return;
			case 'error': {
				this.#settleLive();
				this.#notice(
					'error',
					str(ev.error) ??
						(isRec(ev.error) ? str(ev.error.message) : undefined) ??
						str(ev.errorMessage) ??
						str(ev.reason) ??
						'Assistant stream error',
				);
				return;
			}
			default:
				// image_end, signature_delta, thinking_dropped, redacted_thinking…
				return;
		}
	}

	/** Replace live streaming rows with the authoritative message content,
	 *  preserving tool status/results already delivered by execution events. */
	#reconcileAssistant(msg: RpcMessage): void {
		const live = this.#live;
		this.#live = null;
		let insertAt = this.view.items.length;
		const ids = new Set<string>(this.#staleLiveIds);
		this.#staleLiveIds.clear();
		if (live) for (const b of live.blocks.values()) ids.add(b.id);
		if (ids.size > 0) {
			for (let i = 0; i < this.view.items.length; i++) {
				if (ids.has(this.view.items[i].id)) {
					insertAt = i;
					break;
				}
			}
			for (let i = this.view.items.length - 1; i >= 0; i--) {
				if (ids.has(this.view.items[i].id)) this.view.items.splice(i, 1);
			}
		}

		const rawItems: ConversationItem[] = [];
		const toolRegs: number[] = [];
		for (const b of assistantBlocks(msg)) {
			if (b.type === 'toolCall') {
				const toolCallId = str(b.id) ?? '';
				const existing = toolCallId ? this.#toolIndex.get(toolCallId) : undefined;
				if (existing) {
					existing.toolName = str(b.name) ?? existing.toolName;
					if (isRec(b.arguments)) existing.args = b.arguments;
					if (!this.view.items.includes(existing)) rawItems.push(existing);
					continue;
				}
				const item: ToolItem = {
					id: nid(),
					kind: 'tool',
					toolCallId,
					toolName: str(b.name) ?? 'tool',
					args: isRec(b.arguments) ? b.arguments : {},
					status: 'queued',
					timestamp: msg.timestamp,
				};
				toolRegs.push(rawItems.length);
				rawItems.push(item);
				continue;
			}
			const made = makeTextItem(b, msg.timestamp, false);
			if (made) rawItems.push(made);
		}
		this.view.items.splice(insertAt, 0, ...rawItems);
		for (const ri of toolRegs) {
			const stored = this.view.items[insertAt + ri] as ToolItem;
			if (stored.toolCallId) this.#toolIndex.set(stored.toolCallId, stored);
		}

		if (isRec(msg.usage)) this.#addUsage(msg.usage);
		if (msg.stopReason === 'error') {
			this.#idleStatus = 'failed';
			const em = (msg as unknown as Record<string, unknown>).errorMessage;
			this.#notice('error', str(em) ?? 'Request failed');
		}
	}

	/* ---- tool execution events ---- */

	#onToolStart(frame: Record<string, unknown>): void {
		const id = str(frame.toolCallId);
		const args = isRec(frame.args) ? frame.args : undefined;
		const intent = str(frame.intent);
		const toolName = str(frame.toolName);
		let item = id ? this.#toolIndex.get(id) : undefined;
		if (!item) {
			const raw: ToolItem = {
				id: nid(),
				kind: 'tool',
				toolCallId: id ?? '',
				toolName: toolName ?? 'tool',
				args: args ?? {},
				intent,
				status: 'running',
			};
			this.view.items.push(raw);
			item = this.view.items[this.view.items.length - 1] as ToolItem;
			if (id) this.#toolIndex.set(id, item);
			return;
		}
		item.status = 'running';
		if (args) item.args = args;
		if (intent) item.intent = intent;
		if (toolName) item.toolName = toolName;
	}

	#onToolUpdate(frame: Record<string, unknown>): void {
		const id = str(frame.toolCallId);
		const partial = normalizeResult(frame.partialResult ?? frame.result ?? frame.partial);
		let item = id ? this.#toolIndex.get(id) : undefined;
		if (!item) {
			this.#onToolStart(frame);
			item = id ? this.#toolIndex.get(id) : undefined;
		}
		if (!item) return;
		if (partial) item.partial = partial;
		if (item.status === 'queued') item.status = 'running';
	}

	#onToolEnd(frame: Record<string, unknown>): void {
		const id = str(frame.toolCallId);
		let item = id ? this.#toolIndex.get(id) : undefined;
		if (!item) {
			this.#onToolStart(frame);
			item = id ? this.#toolIndex.get(id) : undefined;
		}
		if (!item) return;
		const result = normalizeResult(frame.result);
		if (result) item.result = result;
		item.partial = undefined;
		item.status = frame.isError === true ? 'failed' : 'completed';
	}

	/* ---- extension UI requests ---- */

	#onUiRequest(frame: Record<string, unknown>): void {
		const method = str(frame.method);
		const id = str(frame.id);
		if (!method) return;
		if (method === 'cancel') {
			this.#removeRequest(str(frame.targetId) ?? id);
			return;
		}
		if (method === 'notify') {
			this.#notice(mapLevel(str(frame.notifyType)), str(frame.message) ?? '');
			return;
		}
		if (method === 'setStatus' || method === 'setTitle') return;
		if (method === 'set_editor_text') {
			const text = str(frame.text);
			if (text) this.#sink.push({ id: nid(), kind: 'custom', customType: 'editor_text', text });
			return;
		}
		if (method === 'setWidget') {
			const lines = arr(frame.widgetLines)
				?.map((l) => str(l) ?? '')
				.filter(Boolean)
				.join('\n');
			if (lines)
				this.#sink.push({
					id: nid(),
					kind: 'custom',
					customType: 'widget',
					text: lines,
					details: clean({ key: str(frame.widgetKey), placement: str(frame.widgetPlacement) }),
				});
			return;
		}
		if (!id) return;
		const req = this.#buildRequest(id, method, frame);
		if (req) {
			this.view.pendingRequests = [...this.view.pendingRequests.filter((r) => r.id !== id), req];
			this.#refreshStatus();
		} else {
			this.#notice('warn', `Unsupported ${method} request: ${str(frame.title) ?? id}`);
		}
	}

	#buildRequest(id: string, method: string, frame: Record<string, unknown>): UiRequest | null {
		const timeout = num(frame.timeout);
		const toolName = str(frame.toolName);
		const toolArgs = isRec(frame.toolArgs) ? frame.toolArgs : undefined;
		const cwd = str(frame.cwd);
		switch (method) {
			case 'select': {
				const title = str(frame.title) ?? '';
				const options = arr(frame.options)
					?.map((o) => str(o) ?? (isRec(o) ? str(o.label) ?? str(o.value) : undefined))
					.filter((o): o is string => typeof o === 'string' && o.length > 0);
				// OMP sends optionDetails as a map keyed by option label;
				// tolerate an array form too.
				const optionDetails = arr(frame.optionDetails)
					?.filter(isRec)
					.map((d) => ({ description: str(d.description) })) ??
					(isRec(frame.optionDetails) && options
						? options.map((o) => {
								const d = (frame.optionDetails as Record<string, unknown>)[o];
								return { description: isRec(d) ? str(d.description) : undefined };
							})
						: undefined);
				if (title.startsWith('Allow tool:')) {
					const nl = title.indexOf('\n');
					const name = (nl < 0 ? title.slice(11) : title.slice(11, nl)).trim();
					const message = nl < 0 ? undefined : title.slice(nl + 1).trim() || undefined;
					return {
						id,
						method: 'permission',
						title,
						message,
						options,
						optionDetails,
						timeout,
						toolName: toolName ?? name,
						toolArgs,
						cwd,
					};
				}
				return { id, method: 'select', title, message: str(frame.message), options, optionDetails, timeout, toolName, toolArgs, cwd };
			}
			case 'confirm':
				return { id, method: 'confirm', title: str(frame.title) ?? '', message: str(frame.message), timeout, toolName, toolArgs, cwd };
			case 'input':
				return {
					id,
					method: 'input',
					title: str(frame.title) ?? '',
					message: str(frame.message),
					placeholder: str(frame.placeholder),
					timeout,
					toolName,
					toolArgs,
					cwd,
				};
			case 'editor':
				return { id, method: 'editor', title: str(frame.title) ?? '', prefill: str(frame.prefill), timeout, toolName, toolArgs, cwd };
			case 'open_url':
				return {
					id,
					method: 'open_url',
					title: str(frame.title) ?? 'Open URL',
					message: str(frame.instructions) ?? str(frame.message),
					url: str(frame.url),
					launchUrl: str(frame.launchUrl),
					timeout,
					toolName,
					toolArgs,
					cwd,
				};
			default:
				return null;
		}
	}

	#removeRequest(id: string | undefined): void {
		if (!id) return;
		const next = this.view.pendingRequests.filter((r) => r.id !== id);
		if (next.length === this.view.pendingRequests.length) return;
		this.view.pendingRequests = next;
		this.#refreshStatus();
	}

	/* ---- subagents ---- */

	#upsertAgent(patch: Partial<AgentInfo> & { id: string }): void {
		const i = this.view.agents.findIndex((a) => a.id === patch.id);
		if (i < 0) {
			this.view.agents.push({ name: patch.id, status: 'pending', ...clean(patch as Record<string, unknown>) } as AgentInfo);
			return;
		}
		const a = this.view.agents[i] as unknown as Record<string, unknown>;
		for (const [k, v] of Object.entries(patch)) {
			if (v !== undefined) a[k] = v;
		}
	}

	#onSubagentLifecycle(frame: Record<string, unknown>): void {
		const p = frame.payload;
		if (!isRec(p)) return;
		const id = str(p.id);
		if (!id) return;
		this.#upsertAgent({
			id,
			parentId: dottedParent(id),
			parentToolCallId: str(p.parentToolCallId),
			name: id.slice(id.lastIndexOf('.') + 1),
			role: str(p.agent) ?? str(p.agentSource),
			task: str(p.task) ?? str(p.assignment),
			status: mapAgentStatus(str(p.status)),
			activity: str(p.description),
			sessionFile: str(p.sessionFile),
		});
	}

	#onSubagentProgress(frame: Record<string, unknown>): void {
		const p = frame.payload;
		if (!isRec(p)) return;
		const prog = isRec(p.progress) ? p.progress : p;
		const id = str(prog.id) ?? str(p.id);
		if (!id) return;
		const currentTool = str(prog.currentTool);
		this.#upsertAgent({
			id,
			parentId: dottedParent(id),
			parentToolCallId: str(prog.parentToolCallId) ?? str(p.parentToolCallId),
			name: id.slice(id.lastIndexOf('.') + 1),
			role: str(p.agent) ?? str(prog.agent) ?? str(p.agentSource) ?? str(prog.agentSource),
			task: str(prog.task) ?? str(prog.assignment),
			status: mapAgentStatus(str(prog.status)),
			activity: str(prog.lastIntent) ?? (currentTool ? `Running ${currentTool}` : undefined) ?? str(prog.description),
			model: str(prog.resolvedModelIdentity) ?? str(prog.resolvedModel) ?? str(prog.modelOverride),
			effort: str(prog.resolvedThinkingLevel) ?? str(prog.effort),
			tokens: num(prog.tokens),
			contextTokens: num(prog.contextTokens),
			contextWindow: num(prog.contextWindow),
			cost: num(prog.cost),
			durationMs: num(prog.durationMs),
			toolCount: num(prog.toolCount),
			sessionFile: str(p.sessionFile) ?? str(prog.sessionFile),
		});
	}

	/* ---- model / usage / misc ---- */

	#onConfigUpdate(frame: Record<string, unknown>): void {
		// Pi shape: { property, previous, value }. OMP shape: { model, thinkingLevel }.
		const prop = str(frame.property);
		if (prop === 'model') {
			const m = normalizeModel(frame.value);
			if (m) this.view.model = m;
			return;
		}
		if (prop === 'thinkingLevel') {
			this.view.effort = str(frame.value) ?? this.view.effort;
			return;
		}
		if (prop) return;
		const m = normalizeModel(frame.model);
		if (m) this.view.model = m;
		const tl = str(frame.thinkingLevel);
		if (tl) this.view.effort = tl;
	}

	#onUsageEvent(frame: Record<string, unknown>): void {
		const totals = isRec(frame.totals) ? frame.totals : undefined;
		if (totals) {
			const u = parseUsage(totals);
			if (u) this.view.usage = u;
			return;
		}
		// Per-row usage is intentionally ignored: assistant message_end usage
		// already accumulates, and adding both would double-count.
	}

	#addUsage(u: Record<string, unknown>): void {
		const cur: Usage =
			this.view.usage ?? { tokens: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 }, cost: 0 };
		cur.tokens.input += num(u.input) ?? 0;
		cur.tokens.output += num(u.output) ?? 0;
		cur.tokens.cacheRead += num(u.cacheRead) ?? 0;
		cur.tokens.cacheWrite += num(u.cacheWrite) ?? 0;
		cur.tokens.total += num(u.totalTokens) ?? num(u.total) ?? 0;
		cur.cost += isRec(u.cost) ? num(u.cost.total) ?? 0 : num(u.cost) ?? 0;
		this.view.usage = cur;
		const ctx = (num(u.input) ?? 0) + (num(u.cacheRead) ?? 0) + (num(u.cacheWrite) ?? 0);
		const cw = this.view.model?.contextWindow ?? 0;
		if (ctx > 0 && cw > 0) {
			const cu: ContextUsage = { tokens: ctx, contextWindow: cw, percent: (ctx / cw) * 100 };
			this.view.contextUsage = cu;
		}
	}

	#onBashUpdate(frame: Record<string, unknown>): void {
		const id = str(frame.id) ?? '_';
		const delta = str(frame.delta) ?? '';
		if (!delta) return;
		const e = this.#bashLive.get(id);
		if (!e) {
			const item = this.#sink.push({
				id: nid(),
				kind: 'custom',
				customType: 'bashExecution',
				text: delta,
				details: { output: delta },
			});
			this.#bashLive.set(id, { item, output: delta });
			return;
		}
		e.output += delta;
		(e.item as TextItem).text = e.output;
		const det = (e.item as { details?: unknown }).details;
		if (isRec(det)) det.output = e.output;
	}

	#ingestBash(msg: RpcMessage): void {
		const output = str(msg.output);
		let match: string | undefined;
		for (const [id, e] of this.#bashLive) {
			if (output === undefined || output.startsWith(e.output)) {
				match = id;
				break;
			}
		}
		if (match !== undefined) {
			const e = this.#bashLive.get(match);
			this.#bashLive.delete(match);
			if (e) {
				(e.item as TextItem).text = str(msg.command) ?? (e.item as TextItem).text;
				const details: Record<string, unknown> = { ...(isRec(msg.details) ? msg.details : {}) };
				if (msg.output !== undefined) details.output = msg.output;
				if (msg.exitCode !== undefined) details.exitCode = msg.exitCode;
				(e.item as { details?: unknown }).details = details;
				return;
			}
		}
		this.#sink.pushBash(msg);
	}

	#onEntryAdded(frame: Record<string, unknown>): void {
		const e = frame.entry;
		if (!isRec(e)) return;
		const parsed = typeof e.timestamp === 'string' ? Date.parse(e.timestamp) : NaN;
		const ts = Number.isFinite(parsed) ? parsed : num(e.timestamp);
		switch (str(e.type)) {
			case 'message':
				const em = asMsg(e.message);
				if (em) this.#ingestLive(em);
				return;
			case 'custom_message':
				this.#ingestLive({
					role: 'custom',
					customType: str(e.customType),
					content: e.content as RpcMessage['content'],
					display: e.display === false ? false : undefined,
					details: e.details,
					timestamp: ts,
				});
				return;
			case 'compaction':
				this.#sink.push({
					id: nid(),
					kind: 'custom',
					customType: 'compaction',
					text: str(e.summary) ?? 'Context compacted',
					details: e.details,
					timestamp: ts,
				});
				return;
			case 'branch_summary':
				this.#sink.push({
					id: nid(),
					kind: 'custom',
					customType: 'branchSummary',
					text: str(e.summary) ?? 'Branch summary',
					timestamp: ts,
				});
				return;
			case 'model_change': {
				const provider = str(e.provider);
				const modelId = str(e.modelId);
				if (provider && modelId) this.view.model = { provider, id: modelId, name: modelId };
				return;
			}
			case 'thinking_level_change':
				this.view.effort = str(e.thinkingLevel) ?? this.view.effort;
				return;
			case 'usage':
				// Usage ledger entries parallel assistant message usage; skipped
				// to avoid double-counting (see #onUsageEvent).
				return;
			default:
				return;
		}
	}

	/* ---- helpers ---- */

	#notice(level: string, text: string, details?: unknown): void {
		if (!text) return;
		this.#sink.push({ id: nid(), kind: 'notice', text, level, details });
	}

	#errorText(frame: Record<string, unknown>): string | undefined {
		const e = frame.error;
		return str(e) ?? (isRec(e) ? str(e.message) : undefined) ?? str(frame.message) ?? str(frame.errorMessage);
	}

	#refreshStatus(): void {
		if (this.#failed) {
			this.view.status = 'disconnected';
			return;
		}
		this.view.status = this.view.pendingRequests.length > 0 ? 'waiting' : this.#runActive ? 'active' : this.#idleStatus;
	}
}
