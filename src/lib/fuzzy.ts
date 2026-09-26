/**
 * Rank paths for an `@` query: every query character must appear in order.
 * Matches in the file name, at word starts, and in runs score higher; shorter
 * paths win ties. Returns best first.
 */
export function rankPaths(paths: string[], query: string, limit = 12): string[] {
  const q = query.toLowerCase().replace(/^\.\//, '');
  // `folder/` lists what's directly inside it: subfolders, then files.
  if (q.endsWith('/') && paths.some(path => path.toLowerCase() === q)) return folderContents(paths, q).slice(0, limit);
  if (!q) {
    return [...paths].sort((a, b) => depth(a) - depth(b) || a.length - b.length || a.localeCompare(b)).slice(0, limit);
  }
  const scored: Array<[number, string]> = [];
  for (const path of paths) {
    const score = scorePath(path, q);
    if (score > 0) scored.push([score, path]);
  }
  scored.sort((a, b) => b[0] - a[0] || a[1].length - b[1].length || a[1].localeCompare(b[1]));
  return scored.slice(0, limit).map(([, path]) => path);
}

/** Direct children of `folder/`, folders first, then alphabetically. */
export function folderContents(paths: string[], folder: string): string[] {
  const children = paths.filter(path => {
    const lower = path.toLowerCase();
    if (!lower.startsWith(folder) || lower === folder) return false;
    const rest = path.slice(folder.length);
    const slash = rest.indexOf('/');
    return slash < 0 || slash === rest.length - 1;
  });
  return children.sort((a, b) => Number(b.endsWith('/')) - Number(a.endsWith('/')) || a.localeCompare(b, undefined, { sensitivity: 'base' }));
}

function depth(path: string): number {
  return path.split('/').length - (path.endsWith('/') ? 1 : 0);
}

export function scorePath(path: string, query: string): number {
  const lower = path.toLowerCase();
  const nameStart = lower.lastIndexOf('/', lower.length - 2) + 1;
  const folder = path.endsWith('/');
  if (query.includes('/')) {
    // Typing a path: prefix matches first, nearer levels before deeper ones,
    // then shorter (so the folder leads).
    if (lower.startsWith(query)) {
      const deeper = path.slice(query.lastIndexOf('/') + 1).replace(/\/$/, '').split('/').length - 1;
      return 9_000 - deeper * 200 - path.length;
    }
    const at = lower.indexOf(query);
    if (at >= 0) return 6_000 - at - path.length;
  } else {
    // Typing a name: the file or folder's own name counts most; files edge
    // out folders on otherwise equal matches.
    const name = lower.slice(nameStart).replace(/\/$/, '');
    if (name === query) return 10_000 - path.length;
    const inName = name.indexOf(query);
    if (inName >= 0) return 7_000 + (inName === 0 ? 500 : 0) - path.length - (folder ? 300 : 0);
    const at = lower.indexOf(query);
    if (at >= 0) return 5_000 - at - path.length - (folder ? 300 : 0);
  }
  let score = 0;
  let from = 0;
  let run = 0;
  for (const char of query) {
    const at = lower.indexOf(char, from);
    if (at < 0) return 0;
    const boundary = at === 0 || '/_-. '.includes(lower[at - 1]) || (path[at] >= 'A' && path[at] <= 'Z' && path[at - 1] >= 'a' && path[at - 1] <= 'z');
    run = at === from ? run + 1 : 0;
    score += 10 + run * 8 + (boundary ? 12 : 0) + (at >= nameStart ? 6 : 0);
    from = at + 1;
  }
  return score - path.length * 0.1;
}

/** Files plus every folder above them (as `dir/`), for mention suggestions. */
export function withFolders(files: string[]): string[] {
  const folders = new Set<string>();
  for (const file of files) {
    let at = file.indexOf('/');
    while (at > 0) {
      folders.add(`${file.slice(0, at)}/`);
      at = file.indexOf('/', at + 1);
    }
  }
  return [...folders, ...files];
}

/** The `@query` being typed at the caret, if any. */
export function mentionAt(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const match = /(^|\s)@([^\s@]*)$/.exec(before);
  if (!match) return null;
  return { start: caret - match[2].length - 1, query: match[2] };
}
