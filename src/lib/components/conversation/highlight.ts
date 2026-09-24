import type { BundledLanguage, Highlighter } from 'shiki';

const LIGHT_THEME = 'github-light-default';
const DARK_THEME = 'github-dark-default';

const INITIAL_LANGS = [
  'typescript',
  'javascript',
  'tsx',
  'jsx',
  'json',
  'bash',
  'shell',
  'markdown',
  'python',
  'go',
  'rust',
  'css',
  'html',
  'yaml',
  'toml',
  'diff',
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
 * Highlight a completed code block. Returns null when highlighting is
 * unavailable so callers can fall back to escaped plain text.
 */
export async function highlightCode(code: string, lang?: string): Promise<string | null> {
  const highlighter = await getHighlighter();
  if (!highlighter) return null;
  const normalized = (lang ?? '').trim().toLowerCase();
  const resolved = LANG_ALIASES[normalized] ?? normalized;
  const target = resolved || 'text';
  if (!(await ensureLanguage(highlighter, target))) {
    if (target === 'text') return null;
    return highlightCode(code, 'text');
  }
  try {
    return highlighter.codeToHtml(code, {
      lang: target,
      themes: { light: LIGHT_THEME, dark: DARK_THEME },
      defaultColor: false,
    });
  } catch {
    return null;
  }
}

export function plainCodeHtml(code: string): string {
  const escaped = code.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  return `<pre class="shiki-plain"><code>${escaped}</code></pre>`;
}
