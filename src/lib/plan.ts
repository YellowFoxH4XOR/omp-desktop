import type { UiRequest } from './types';

/** Labels shared with src-tauri/src/pidesk-modes.mjs (request_auto). */
export const PLAN_TITLE = 'Run this plan in Auto mode?';
export const PLAN_APPROVE = 'Approve and run in Auto';
export const PLAN_FEEDBACK = 'Revise with feedback';
export const PLAN_DECLINE = 'Decline';

/** The plan Markdown when `request` is a Plan → Auto approval, else null. */
export function planOf(request: UiRequest | undefined): string | null {
  if (!request || request.method !== 'select') return null;
  const [first, ...rest] = request.title.split('\n');
  if (first !== PLAN_TITLE || !request.options?.includes(PLAN_APPROVE)) return null;
  return rest.join('\n').trim();
}

export type PlanOutcome = 'approved' | 'revising' | 'declined' | 'pending';

/** Classify a finished request_auto tool result for the transcript card. */
export function planOutcome(result: string): PlanOutcome {
  if (/^(Approved|Already approved|Auto mode is already on)/.test(result)) return 'approved';
  if (result.startsWith('The user wants changes')) return 'revising';
  if (result.startsWith('The user declined')) return 'declined';
  return 'pending';
}

export interface PlanActions {
  plan: string;
  onApprove: () => void | Promise<void>;
  onDecline: () => void | Promise<void>;
  onFeedback: (text: string) => void | Promise<void>;
  /** Opens the full plan review panel. */
  onReview?: () => void;
}

const RUNNERS = /^(bun|bunx|npm|npx|pnpm|yarn|cargo|git|make|pytest|go|deno|node|python3?|uv|docker)\b/;
const PATHISH = /^[\w@.~-]*(\/[\w@.-]+)+\/?$|^[\w@-]+\.[a-z0-9]{1,6}$/i;

/** Quick facts for the approval strip: step count, files named, first command. */
export function planFacts(plan: string): { steps: number; files: number; command?: string } {
  const steps = plan.split('\n').filter(line => /^\s*\d+[.)]\s+\S/.test(line)).length;
  const spans = [...plan.matchAll(/`([^`\n]{1,160})`/g)].map(match => match[1].trim());
  const files = new Set(spans.filter(span => PATHISH.test(span) && !RUNNERS.test(span)));
  return { steps, files: files.size, command: spans.find(span => RUNNERS.test(span) && span.includes(' ')) };
}
