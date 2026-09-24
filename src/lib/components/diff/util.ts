import { HighlightStyle, LanguageDescription, type LanguageSupport } from '@codemirror/language';
import { languages } from '@codemirror/language-data';
import { EditorView } from '@codemirror/view';
import { tags as t } from '@lezer/highlight';

export type EolKind = 'lf' | 'crlf' | 'mixed' | 'none';

/** Detect the dominant line ending of raw file text. 'mixed' means both LF and CRLF appear. */
export function detectEol(text: string): EolKind {
  let sawLf = false;
  let sawCrlf = false;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c === 13) {
      if (text.charCodeAt(i + 1) === 10) {
        sawCrlf = true;
        i++;
      }
    } else if (c === 10) {
      sawLf = true;
    }
    if (sawLf && sawCrlf) return 'mixed';
  }
  if (sawLf && sawCrlf) return 'mixed';
  if (sawCrlf) return 'crlf';
  if (sawLf) return 'lf';
  return 'none';
}

/** Resolve a language description for a path and lazily load its grammar. */
export async function languageFor(path: string): Promise<LanguageSupport | null> {
  const name = path.split('/').pop() || path;
  const desc = LanguageDescription.matchFilename(languages, name);
  if (!desc) return null;
  try {
    const support = await desc.load();
    return support;
  } catch {
    return null;
  }
}

export interface StatusMeta {
  code: string;
  cls: 'added' | 'modified' | 'deleted' | 'renamed' | 'conflict' | 'other';
  label: string;
}

/** Map a git status string (porcelain code or word) to a compact badge. */
export function statusMeta(status: string): StatusMeta {
  const s = status.trim().toLowerCase();
  const first = s.charAt(0);
  if (s.includes('conflict') || s === 'u' || s === 'uu' || s === 'aa' || s === 'dd') {
    return { code: '!', cls: 'conflict', label: 'conflict' };
  }
  if (first === 'a' || s === '??' || s === 'untracked' || s === 'new' || s === 'new file' || s === 'added') {
    return { code: 'A', cls: 'added', label: 'added' };
  }
  if (first === 'd' || s === 'deleted') {
    return { code: 'D', cls: 'deleted', label: 'deleted' };
  }
  if (first === 'r' || s === 'renamed') {
    return { code: 'R', cls: 'renamed', label: 'renamed' };
  }
  if (first === 'c' || s === 'copied' || s === 'typechange' || first === 't') {
    return { code: 'T', cls: 'modified', label: 'type change' };
  }
  return { code: 'M', cls: 'modified', label: 'modified' };
}

/** Join a repo-relative path onto a working directory root. */
export function joinPath(root: string, rel: string): string {
  if (!root) return rel;
  const sep = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  const base = root.endsWith('/') || root.endsWith('\\') ? root.slice(0, -1) : root;
  return `${base}${sep}${rel}`;
}

export function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === 'string') return err;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

/** Syntax colors tuned for the app's dark surface tokens. */
export const diffHighlight = HighlightStyle.define([
  { tag: [t.keyword, t.modifier, t.controlKeyword, t.operatorKeyword], color: '#c792ea' },
  { tag: [t.string, t.special(t.string), t.regexp], color: '#c3e88d' },
  { tag: [t.number, t.bool, t.null, t.atom], color: '#f78c6c' },
  { tag: [t.comment, t.blockComment, t.lineComment, t.docComment], color: '#697098', fontStyle: 'italic' },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: '#82aaff' },
  { tag: [t.typeName, t.className, t.tagName, t.standard(t.typeName)], color: '#ffcb6b' },
  { tag: [t.propertyName, t.attributeName], color: '#addb67' },
  { tag: [t.variableName, t.definition(t.variableName)], color: '#d6deeb' },
  { tag: [t.operator, t.punctuation, t.separator, t.derefOperator], color: '#89ddff' },
  { tag: [t.meta, t.annotation, t.processingInstruction], color: '#7fdbca' },
  { tag: t.heading, color: '#82b1ff', fontWeight: 'bold' },
  { tag: t.link, color: '#82aaff', textDecoration: 'underline' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strong, fontWeight: 'bold' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  { tag: t.invalid, color: '#ff5370' },
]);

/**
 * Base editor theme plus merge-view overrides. The merge package ships its own
 * base theme (orange/green tints + underline gradients); we restyle changed
 * regions with the app's good/bad tokens so diffs read as add/remove.
 */
export const diffTheme = EditorView.theme(
  {
    '&': {
      backgroundColor: 'var(--bg)',
      color: 'var(--text)',
      fontSize: '12px',
      height: '100%',
    },
    '.cm-scroller': {
      fontFamily: "ui-monospace, 'SF Mono', Menlo, Consolas, monospace",
      lineHeight: '1.55',
      overflow: 'auto',
    },
    '.cm-content': { padding: '4px 0' },
    '.cm-gutters': {
      backgroundColor: 'var(--bg)',
      color: 'var(--muted)',
      border: 'none',
      borderRight: '1px solid var(--line)',
    },
    '.cm-lineNumbers .cm-gutterElement': { padding: '0 6px 0 10px', minWidth: '2.4em' },
    '.cm-activeLine': { backgroundColor: 'transparent' },
    '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': {
      backgroundColor: 'color-mix(in srgb, var(--accent) 26%, transparent) !important',
    },
    '.cm-cursor': { borderLeftColor: 'var(--text)' },
    // Merge view chrome
    '.cm-mergeView': { height: '100%', overflow: 'auto' },
    '.cm-mergeViewEditor': { overflow: 'hidden' },
    '.cm-merge-revert': {
      width: '22px',
      backgroundColor: 'var(--bg)',
      borderLeft: '1px solid var(--line)',
      borderRight: '1px solid var(--line)',
    },
    '.cm-merge-revert button': {
      color: 'var(--muted)',
      fontSize: '13px',
      padding: '1px 0',
    },
    '.cm-merge-revert button:hover': { color: 'var(--bad)' },
    // Changed-line backgrounds: A side (HEAD) reads as removed, B side as added.
    '&.cm-merge-a .cm-changedLine, .cm-deletedChunk': {
      backgroundColor: 'color-mix(in srgb, var(--bad) 9%, transparent)',
    },
    '&.cm-merge-b .cm-changedLine, .cm-inlineChangedLine': {
      backgroundColor: 'color-mix(in srgb, var(--good) 9%, transparent)',
    },
    // Word-level highlights replace the package's underline gradients.
    '&.cm-merge-a .cm-changedText, & .cm-deletedChunk .cm-deletedText': {
      background: 'color-mix(in srgb, var(--bad) 32%, transparent)',
      borderRadius: '2px',
    },
    '&.cm-merge-b .cm-changedText': {
      background: 'color-mix(in srgb, var(--good) 30%, transparent)',
      borderRadius: '2px',
    },
    '.cm-deletedChunk del': { textDecoration: 'none' },
    // Change gutter markers
    '.cm-changeGutter .cm-gutterElement': { padding: '0 2px' },
    '.cm-deletedLineGutter': { backgroundColor: 'var(--bad)' },
    '.cm-changedLineGutter': { backgroundColor: 'var(--good)' },
    // Collapsed unchanged regions
    '.cm-collapsedLines': {
      padding: '2px 8px',
      color: 'var(--muted)',
      backgroundColor: 'var(--surface)',
      borderTop: '1px solid var(--line)',
      borderBottom: '1px solid var(--line)',
      cursor: 'pointer',
      fontSize: '11px',
    },
    // Chunk action buttons (unified view)
    '.cm-chunkButtons': {
      display: 'flex',
      justifyContent: 'flex-end',
      gap: '4px',
      padding: '1px 6px 1px 0',
    },
    '.cm-chunkButtons button, .cmp-hunk-revert': {
      font: 'inherit',
      fontSize: '10px',
      lineHeight: '1.4',
      padding: '1px 8px',
      borderRadius: '4px',
      border: '1px solid var(--line)',
      backgroundColor: 'var(--surface-2)',
      color: 'var(--text)',
      cursor: 'pointer',
    },
    '.cm-chunkButtons button:hover, .cmp-hunk-revert:hover': {
      borderColor: 'var(--bad)',
      color: 'var(--bad)',
    },
  },
  { dark: true },
);
