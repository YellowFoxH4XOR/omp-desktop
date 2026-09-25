import { marked, type Token, type Tokens, type TokensList } from 'marked';
import DOMPurify from 'dompurify';

export interface MdSegment {
  kind: 'html' | 'code';
  /** Sanitized HTML for kind 'html'. */
  html?: string;
  /** Raw code text for kind 'code'. */
  code?: string;
  /** Info string language for kind 'code'. */
  lang?: string;
  /** True when the fence is closed (or the message is finished). */
  complete?: boolean;
}

const PROSE_FORBID_TAGS = [
  'style',
  'form',
  'input',
  'button',
  'textarea',
  'select',
  'option',
  'iframe',
  'object',
  'embed',
  'audio',
  'video',
  'svg',
  'math',
  'base',
  'link',
  'meta',
  // Never fetch model-supplied images from remote hosts in a local-first app.
  'img',
];

/// Model-authored HTML: no `style` attribute, so agent output cannot overlay
/// or spoof the app's own dialogs.
const PURIFY_CONFIG = {
  FORBID_TAGS: PROSE_FORBID_TAGS,
  FORBID_ATTR: ['srcdoc', 'formaction', 'style'],
};

/// Highlighter output only: Shiki emits `style="--shiki-light:…;--shiki-dark:…"`
/// for dual themes, so this path keeps `style` while restricting it to those
/// custom properties (see [`restrict_shiki_styles`]).
const HIGHLIGHT_PURIFY_CONFIG = {
  FORBID_TAGS: PROSE_FORBID_TAGS,
  FORBID_ATTR: ['srcdoc', 'formaction'],
};

const SHIKI_STYLE_ATTR = /style="([^"]*)"/g;
const SHIKI_DECLARATION = /^--shiki-[a-z-]+:\s*[^;]+$/;

/// Keep only Shiki's theme custom properties from any style attribute, so a
/// highlighter change can never reintroduce layout/spoofing CSS.
function restrict_shiki_styles(html: string): string {
  return html.replace(SHIKI_STYLE_ATTR, (_match, declarations: string) => {
    const kept = declarations
      .split(';')
      .map((declaration) => declaration.trim())
      .filter((declaration) => SHIKI_DECLARATION.test(declaration));
    return kept.length > 0 ? `style="${kept.join(';')}"` : '';
  });
}

let linkHookInstalled = false;

function ensureLinkHook(): void {
  if (linkHookInstalled) return;
  linkHookInstalled = true;
  // External links must never navigate the webview itself; they are opened
  // through the opener plugin from a delegated click handler.
  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (node.tagName === 'A') {
      node.setAttribute('target', '_blank');
      node.setAttribute('rel', 'noopener noreferrer');
    }
  });
}

export function sanitizeHtml(html: string): string {
  ensureLinkHook();
  return DOMPurify.sanitize(html, PURIFY_CONFIG);
}

/// Sanitize highlighter output: same policy as prose, plus Shiki's theme
/// custom properties (`style="--shiki-light:…"`) which the dual-theme markup
/// needs. Every other declaration is dropped.
export function sanitizeHighlightHtml(html: string): string {
  ensureLinkHook();
  return restrict_shiki_styles(DOMPurify.sanitize(html, HIGHLIGHT_PURIFY_CONFIG));
}

const CLOSING_FENCE = /(`{3,}|~{3,})[ \t]*$/;

/** Virtualized rows remount on scroll; completed text re-lexed and re-sanitized
 *  each time is the dominant scroll cost in long transcripts. LRU by size. */
const SEGMENT_CACHE_BUDGET_CHARS = 4_000_000;
const segmentCache = new Map<string, { segments: readonly MdSegment[]; size: number }>();
let segmentCacheChars = 0;

function segmentsSize(text: string, segments: readonly MdSegment[]): number {
  let size = text.length;
  for (const segment of segments) size += (segment.html?.length ?? 0) + (segment.code?.length ?? 0);
  return size;
}

function cachedSegments(text: string): readonly MdSegment[] {
  const hit = segmentCache.get(text);
  if (hit) {
    segmentCache.delete(text);
    segmentCache.set(text, hit);
    return hit.segments;
  }
  const segments = parseSegments(text, false);
  const size = segmentsSize(text, segments);
  if (size <= SEGMENT_CACHE_BUDGET_CHARS / 8) {
    segmentCache.set(text, { segments, size });
    segmentCacheChars += size;
    for (const [key, entry] of segmentCache) {
      if (segmentCacheChars <= SEGMENT_CACHE_BUDGET_CHARS) break;
      segmentCache.delete(key);
      segmentCacheChars -= entry.size;
    }
  }
  return segments;
}

/**
 * Split markdown into segments: sanitized HTML for prose and raw code for
 * fenced blocks (which are highlighted lazily by CodeBlock). Completed text is
 * cached, so callers must treat the returned array and segments as read-only.
 */
export function markdownSegments(text: string, streaming = false): readonly MdSegment[] {
  return streaming ? parseSegments(text, true) : cachedSegments(text);
}

function parseSegments(text: string, streaming: boolean): MdSegment[] {
  let tokens: TokensList;
  try {
    tokens = marked.lexer(text);
  } catch {
    return [
      {
        kind: 'html',
        html: `<p>${text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')}</p>`,
      },
    ];
  }

  const segments: MdSegment[] = [];
  let buffer: Token[] = [];

  const flush = () => {
    if (buffer.length === 0) return;
    const slice = buffer.slice() as TokensList;
    slice.links = tokens.links ?? {};
    buffer = [];
    let html: string;
    try {
      html = marked.parser(slice) as string;
    } catch {
      return;
    }
    const clean = sanitizeHtml(html);
    if (clean.trim()) segments.push({ kind: 'html', html: clean });
  };

  for (const token of tokens) {
    if (token.type === 'code') {
      flush();
      const codeToken = token as Tokens.Code;
      const raw = codeToken.raw ?? '';
      const closed = CLOSING_FENCE.test(raw.trimEnd());
      segments.push({
        kind: 'code',
        code: codeToken.text ?? '',
        lang: (codeToken.lang ?? '').trim() || undefined,
        complete: !streaming || closed,
      });
    } else {
      buffer.push(token);
    }
  }
  flush();
  return segments;
}

