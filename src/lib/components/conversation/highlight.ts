import type { BundledLanguage, Highlighter } from 'shiki';
import { sanitizeHighlightHtml } from './markdown';

const LIGHT_THEME = 'github-light-default';
const DARK_THEME = 'github-dark-default';

const INITIAL_LANGS = [
  'typescript',
  'javascript',
  'json',
  'bash',
  'markdown',
  'python',
  'go',
  'rust',
];

const LANG_ALIASES: Record<string, string> = {
  ts: 'typescript',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  py: 'python',
  sh: 'bash',
  zsh: 'bash',
  shellsession: 'bash',
  console: 'bash',
  yml: 'yaml',
  md: 'markdown',
  rs: 'rust',
  golang: 'go',
  'c++': 'cpp',
  'objective-c': 'objc',
  'shellscript': 'bash',
  'c#': 'csharp',
  'f#': 'fsharp',
  'visual-basic': 'vb',
  plaintext: 'text',
  plain: 'text',
  txt: 'text',
  text: 'text',
};

let highlighterPromise: Promise<Highlighter | null> | null = null;
const loadedLangs = new Set<string>();
const pendingLangs = new Map<string, Promise<boolean>>();

/** Tokenizing very large blocks blocks the main thread; show them plain. */
const MAX_HIGHLIGHT_CHARS = 200_000;
/** Virtualized rows remount on scroll, so cache sanitized output (LRU by size). */
const CACHE_BUDGET_CHARS = 4_000_000;
const htmlCache = new Map<string, string>();
let cachedChars = 0;

function cacheGet(key: string): string | undefined {
  const html = htmlCache.get(key);
  if (html === undefined) return undefined;
  htmlCache.delete(key);
  htmlCache.set(key, html);
  return html;
}

function cacheSet(key: string, html: string): void {
  const size = key.length + html.length;
  if (size > CACHE_BUDGET_CHARS / 8) return;
  htmlCache.set(key, html);
  cachedChars += size;
  for (const [oldKey, oldHtml] of htmlCache) {
    if (cachedChars <= CACHE_BUDGET_CHARS) break;
    htmlCache.delete(oldKey);
    cachedChars -= oldKey.length + oldHtml.length;
  }
}

async function getHighlighter(): Promise<Highlighter | null> {
  if (!highlighterPromise) {
    highlighterPromise = (async () => {
      try {
        // Dynamic import is required: Shiki must stay out of the initial
        // bundle and load lazily on first completed code block (PRD §27).
        const shiki = await import('shiki');
        const { createJavaScriptRegexEngine } = await import('shiki/engine/javascript');
        const highlighter = await shiki.createHighlighter({
          themes: [LIGHT_THEME, DARK_THEME],
          langs: INITIAL_LANGS,
          engine: createJavaScriptRegexEngine(),
        });
        for (const lang of INITIAL_LANGS) loadedLangs.add(lang);
        return highlighter;
      } catch {
        return null;
      }
    })();
  }
  return highlighterPromise;
}

async function ensureLanguage(highlighter: Highlighter, lang: string): Promise<boolean> {
  if (lang === 'text' || loadedLangs.has(lang)) return true;
  const pending = pendingLangs.get(lang);
  if (pending) return pending;
  const task = (async () => {
    try {
      await highlighter.loadLanguage(lang as BundledLanguage);
      loadedLangs.add(lang);
      return true;
    } catch {
      return false;
    } finally {
      pendingLangs.delete(lang);
    }
  })();
  pendingLangs.set(lang, task);
  return task;
}

/**
 * Highlight a completed code block into sanitized HTML. Returns null when
 * highlighting is unavailable so callers can fall back to escaped plain text.
 */
export async function highlightCode(code: string, lang?: string): Promise<string | null> {
  const normalized = (lang ?? '').trim().toLowerCase();
  const resolved = LANG_ALIASES[normalized] ?? normalized;
  const target = resolved || 'text';
  // Plain text needs no grammar and must not pull the highlighter into memory.
  if (target === 'text' || code.length > MAX_HIGHLIGHT_CHARS) return null;
  const key = `${target}\0${code}`;
  const cached = cacheGet(key);
  if (cached !== undefined) return cached;

  const highlighter = await getHighlighter();
  if (!highlighter) return null;
  if (!(await ensureLanguage(highlighter, target))) return null;
  try {
    const html = sanitizeHighlightHtml(
      highlighter.codeToHtml(code, {
        lang: target,
        themes: { light: LIGHT_THEME, dark: DARK_THEME },
        defaultColor: false,
      }),
    );
    cacheSet(key, html);
    return html;
  } catch {
    return null;
  }
}

export function plainCodeHtml(code: string): string {
  const escaped = code.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return `<pre class="shiki-plain"><code>${escaped}</code></pre>`;
}
