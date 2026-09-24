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

const PURIFY_CONFIG = {
  FORBID_TAGS: [
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
  ],
  FORBID_ATTR: ['srcdoc', 'formaction'],
};

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

const CLOSING_FENCE = /(`{3,}|~{3,})[ \t]*$/;

/**
 * Split markdown into segments: sanitized HTML for prose and raw code for
 * fenced blocks (which are highlighted lazily by CodeBlock).
 */
export function markdownSegments(text: string, streaming = false): MdSegment[] {
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

