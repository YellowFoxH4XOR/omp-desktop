import DOMPurify from 'dompurify';
import type { Mermaid } from 'mermaid';

/** Diagrams larger than this are shown as code; layout cost grows fast. */
export const MAX_MERMAID_CHARS = 50_000;
const CACHE_LIMIT = 32;

let loader: Promise<Mermaid> | null = null;
let queue: Promise<unknown> = Promise.resolve();
let seq = 0;
const cache = new Map<string, string>();

/** Diagram source is model-authored: SVG only, no scripts, links, images, or HTML islands. */
const SVG_PURIFY_CONFIG = {
  USE_PROFILES: { svg: true, svgFilters: true },
  ADD_TAGS: ['style'],
  FORBID_TAGS: ['script', 'foreignObject', 'image', 'a', 'use', 'iframe'],
  FORBID_ATTR: ['href', 'xlink:href', 'onload', 'onerror', 'onclick'],
};

function load(): Promise<Mermaid> {
  // Dynamic import keeps Mermaid out of the initial bundle until a diagram appears.
  loader ??= import('mermaid').then((module) => module.default);
  return loader;
}

export function isDarkTheme(): boolean {
  const forced = document.documentElement.dataset.theme;
  if (forced === 'dark') return true;
  if (forced === 'light') return false;
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

/**
 * Render Mermaid source to sanitized SVG markup. Rejects with a readable
 * message on syntax errors. Renders are serialized because Mermaid's
 * configuration and scratch DOM are global.
 */
export function renderMermaid(code: string, dark: boolean): Promise<string> {
  const key = `${dark ? 'd' : 'l'}\0${code}`;
  const hit = cache.get(key);
  if (hit !== undefined) {
    cache.delete(key);
    cache.set(key, hit);
    return Promise.resolve(hit);
  }
  const task = queue.then(async () => {
    const mermaid = await load();
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      htmlLabels: false,
      theme: dark ? 'dark' : 'neutral',
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font') || 'system-ui',
    });
    const id = `pidesk-mermaid-${++seq}`;
    try {
      const { svg } = await mermaid.render(id, code);
      const clean = DOMPurify.sanitize(svg, SVG_PURIFY_CONFIG);
      cache.set(key, clean);
      while (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
      return clean;
    } finally {
      // A failed render can leave Mermaid's scratch container in the body.
      document.getElementById(id)?.remove();
      document.getElementById(`d${id}`)?.remove();
    }
  });
  queue = task.catch(() => undefined);
  return task;
}
