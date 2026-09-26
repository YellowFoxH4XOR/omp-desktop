import { expect, test } from 'vitest';
import { mentionAt, rankPaths, withFolders } from './fuzzy';

const files = ['src/App.svelte', 'src/app.css', 'src/lib/api.ts', 'src/lib/components/conversation/Composer.svelte', 'README.md', 'docs/modes.md', 'src-tauri/src/threads.rs'];

test('fuzzy ranking prefers file names, word starts, and short paths', () => {
  expect(rankPaths(files, 'appsv')[0]).toBe('src/App.svelte');
  expect(rankPaths(files, 'composer')[0]).toBe('src/lib/components/conversation/Composer.svelte');
  expect(rankPaths(files, 'thr')[0]).toBe('src-tauri/src/threads.rs');
  expect(rankPaths(files, 'api.ts')).toEqual(['src/lib/api.ts']);
  expect(rankPaths(files, 'zzz')).toEqual([]);
  expect(rankPaths(files, '', 2)).toEqual(['README.md', 'src/app.css']);
});

test('folders come from file paths and can be mentioned', () => {
  const all = withFolders(files);
  expect(all).toContain('src/lib/components/conversation/');
  expect(rankPaths(all, 'docs')[0]).toBe('docs/');
  expect(rankPaths(all, 'docs/')).toEqual(['docs/modes.md']);
});

test('a folder query lists what is directly inside it, folders first', () => {
  const all = withFolders(files);
  expect(rankPaths(all, 'src/lib/')).toEqual(['src/lib/components/', 'src/lib/api.ts']);
  expect(rankPaths(all, 'src/')).toEqual(['src/lib/', 'src/app.css', 'src/App.svelte']);
  // Typing within a folder still prefers its own level.
  expect(rankPaths(all, 'src/lib/c')[0]).toBe('src/lib/components/');
});

test('a mention is the @word at the caret, not an email or finished word', () => {
  expect(mentionAt('look at @src/Ap', 15)).toEqual({ start: 8, query: 'src/Ap' });
  expect(mentionAt('@', 1)).toEqual({ start: 0, query: '' });
  expect(mentionAt('mail me@example.com', 19)).toBeNull();
  expect(mentionAt('@src/app.ts done', 16)).toBeNull();
  expect(mentionAt('line one\n@re', 12)).toEqual({ start: 9, query: 're' });
});
