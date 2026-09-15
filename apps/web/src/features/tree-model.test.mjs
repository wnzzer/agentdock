import test from 'node:test';
import assert from 'node:assert/strict';
import { flattenTree, isTreeChild, sortTreeEntries, treeAncestors, treeParent } from './tree-model.ts';

const file = (path, kind = 'file') => ({ path, name: path.split('/').at(-1), kind, size: 10 });
const directory = entries => ({ entries, loaded: true, loading: false, error: '' });
function fixture() {
  return new Map([
    ['', directory([file('src', 'directory'), file('media', 'directory'), file('README.md')])],
    ['src', directory([file('src/components', 'directory'), file('src/main.ts')])],
    ['src/components', directory([file('src/components/App.vue')])],
    ['media', directory([file('media/Sample video.mp4')])],
  ]);
}

test('the tree projects expanded directories recursively, without changing expansion state', () => {
  const expanded = new Set(['src', 'src/components']);
  const rows = flattenTree(fixture(), expanded);
  assert.deepEqual(rows.map(row => [row.entry.path, row.depth, row.parent]), [
    ['src', 0, ''], ['src/components', 1, 'src'], ['src/components/App.vue', 2, 'src/components'], ['src/main.ts', 1, 'src'], ['media', 0, ''], ['README.md', 0, ''],
  ]);
  assert.deepEqual([...expanded], ['src', 'src/components']);
});

test('collapsing hides descendants while retaining their cached data and child expansion', () => {
  const directories = fixture();
  const expanded = new Set(['src/components']);
  assert.deepEqual(flattenTree(directories, expanded).map(row => row.entry.path), ['src', 'media', 'README.md']);
  expanded.add('src');
  assert.ok(flattenTree(directories, expanded).some(row => row.entry.path === 'src/components/App.vue'));
});

test('keyboard/ARIA projection uses visible sibling counts and positions, not flat list offsets', () => {
  const rows = flattenTree(fixture(), new Set(['src', 'src/components']));
  assert.deepEqual(rows.map(row => [row.position, row.siblings]), [[1, 3], [1, 2], [1, 1], [2, 2], [2, 3], [3, 3]]);
});

test('filtering reveals loaded matches through collapsed ancestors without mutating the layout', () => {
  const expanded = new Set();
  const rows = flattenTree(fixture(), expanded, 'APP.vUE');
  assert.deepEqual(rows.map(row => row.entry.path), ['src', 'src/components', 'src/components/App.vue']);
  assert.deepEqual(rows.map(row => row.expanded), [true, true, false]);
  assert.equal(expanded.size, 0);
  assert.ok(rows.every(row => row.position === 1 && row.siblings === 1));
});

test('filtering only examines loaded, reachable entries; unloaded folders are not invented', () => {
  const directories = fixture();
  directories.delete('src/components');
  directories.set('removed', directory([file('removed/Secret.vue')]));
  assert.deepEqual(flattenTree(directories, new Set(), 'vue'), []);
  assert.deepEqual(flattenTree(directories, new Set(), 'Secret'), []);
  assert.deepEqual(flattenTree(directories, new Set(), 'src/components').map(row => row.entry.path), ['src', 'src/components']);
});

test('a whitespace-only filter is empty and path context can disambiguate equal names', () => {
  assert.deepEqual(flattenTree(fixture(), new Set(), '   ').map(row => row.entry.path), ['src', 'media', 'README.md']);
  assert.deepEqual(flattenTree(fixture(), new Set(), 'media/Sample').map(row => row.entry.path), ['media', 'media/Sample video.mp4']);
});

test('reveal ancestors preserve filename characters and never require sibling scans', () => {
  assert.deepEqual(treeAncestors('src/components/App.vue'), ['', 'src', 'src/components']);
  assert.deepEqual(treeAncestors('README.md'), ['']);
  assert.deepEqual(treeAncestors('资源/视频 #1/测试?.mp4'), ['', '资源', '资源/视频 #1']);
  assert.equal(treeParent('README.md'), '');
  assert.equal(treeParent('src/main.ts'), 'src');
});

test('reveal refuses absolute paths, traversal and ambiguous segments', () => {
  for (const path of ['', '/etc/passwd', '../private', 'src/../private', 'src/./file', 'src//file', 'src/', 'src/\0file']) assert.throws(() => treeAncestors(path));
});

test('a symlink remains a leaf and malformed directory entries cannot create tree cycles', () => {
  const directories = new Map([
    ['', directory([file('link', 'symlink'), file('safe', 'directory'), file('../outside', 'directory')])],
    ['link', directory([file('link/private')])],
    ['safe', directory([file('safe', 'directory'), file('safe/ok.ts')])],
  ]);
  assert.deepEqual(flattenTree(directories, new Set(['link', 'safe'])).map(row => row.entry.path), ['link', 'safe', 'safe/ok.ts']);
  assert.equal(isTreeChild('src', 'src/x.ts'), true);
  assert.equal(isTreeChild('src', 'src2/x.ts'), false);
  assert.equal(isTreeChild('src', 'src/a/x.ts'), false);
});

test('natural directory-first sorting does not mutate the API response', () => {
  const files = [file('part10.ts'), file('z', 'directory'), file('part2.ts'), file('A', 'directory')];
  assert.deepEqual(sortTreeEntries(files).map(row => row.path), ['A', 'z', 'part2.ts', 'part10.ts']);
  assert.equal(files[0].path, 'part10.ts');
});
