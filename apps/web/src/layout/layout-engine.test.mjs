// Node with tsx runs the actual TypeScript source, without a copied implementation.
// Run: pnpm test
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  activatePane, applyPreset, cloneNode, closePane, collapseBelowMin, containerId,
  createDefaultLayoutDocument, createPane, dockPane, findNode, findNodes,
  flattenPanes, isPaneNode, normalizeRatio, projectLayout, replacePane, resizeSplit,
  restoreCollapsed, selectionGroups, snapRatio, splitPane, updatePane, validateLayout,
} from './layout-engine.ts';

const pane = (id) => createPane(id, 'agent_chat', `Session ${id}`, { session_id: id, input: `draft-${id}` });
const split = (id, first, second, direction = 'horizontal', ratio = 0.5) => ({ type: 'split', id, direction, ratio, first, second });
const stack = (id, panes, activePaneId = panes[0]?.id) => ({ type: 'stack', kind: 'stack', id, panes, ...(activePaneId ? { activePaneId } : {}) });
const sortedIds = (root) => flattenPanes(root).map((item) => item.id).sort();

test('default layout prioritizes Agent and first-class Git, not terminal', () => {
  const initial = createDefaultLayoutDocument();
  assert.equal(validateLayout(initial), true);
  assert.deepEqual(flattenPanes(initial.root).map((item) => item.kind), ['agent_chat', 'git_diff']);
});

test('nested horizontal/vertical splitting keeps every pane and unique IDs', () => {
  let root = pane('a');
  for (let index = 0; index < 30; index++) root = splitPane(root, 'a', index % 2 ? 'vertical' : 'horizontal');
  assert.equal(findNodes(root).filter((item) => item.type === 'split').length, 30);
  assert.deepEqual(sortedIds(root), ['a']);
  assert.equal(validateLayout(root), true);
});

test('splitting a tab splits its group without losing inactive tabs', () => {
  const root = splitPane(stack('tabs', [pane('a'), pane('b')], 'b'), 'b', 'vertical');
  assert.equal(root.type, 'split');
  assert.equal(root.direction, 'vertical');
  assert.equal(root.first.type, 'stack');
  assert.equal(root.first.activePaneId, 'b');
  assert.deepEqual(sortedIds(root), ['a', 'b']);
});

test('resize targets deeply nested split and preserves live free ratios', () => {
  const original = split('outer', pane('a'), split('inner', pane('b'), pane('c')));
  const next = resizeSplit(original, 'inner', 0.413);
  assert.equal(next.ratio, 0.5);
  assert.equal(next.second.ratio, 0.413);
  assert.equal(original.second.ratio, 0.5);
  assert.equal(resizeSplit(next, 'missing', 0.2), next);
});

test('ratio snapping is near a rational point, not forced for every drag', () => {
  assert.equal(snapRatio(0.49), 0.5);
  assert.equal(snapRatio(0.34), 1 / 3);
  assert.equal(snapRatio(0.67), 2 / 3);
  assert.equal(snapRatio(0.413), 0.413);
  assert.equal(normalizeRatio(NaN), 0.5);
  assert.equal(normalizeRatio(-1), 0.05);
  assert.equal(normalizeRatio(2), 0.95);
});

test('center docking turns panes into actual tabs with correct activation', () => {
  const a = pane('a');
  const next = dockPane(a, pane('b'), 'a', 'center');
  assert.equal(next.type, 'stack');
  assert.equal(next.activePaneId, 'b');
  assert.deepEqual(sortedIds(next), ['a', 'b']);
  assert.equal(containerId(next, 'b'), next.id);
  assert.equal(activatePane(next, 'a').activePaneId, 'a');
  assert.equal(a.type, 'pane');
});

test('binding an unbound pane replaces it in place and updates its active tab ID', () => {
  const original = split('s', stack('tabs', [createPane('empty-agent', 'agent_chat'), pane('a')], 'empty-agent'), pane('b'), 'horizontal', 0.62);
  const next = replacePane(original, 'empty-agent', pane('real-session'));
  assert.equal(next.ratio, 0.62);
  assert.equal(next.first.activePaneId, 'real-session');
  assert.deepEqual(sortedIds(next), ['a', 'b', 'real-session']);
  assert.equal(next.first.panes[0].metadata.session_id, 'real-session');
  assert.equal(findNode(original, 'empty-agent').id, 'empty-agent');
  assert.equal(validateLayout(next), true);
  assert.equal(replacePane(next, 'a', pane('b')), next);
});

for (const [position, direction, incomingFirst] of [['left', 'horizontal', true], ['right', 'horizontal', false], ['top', 'vertical', true], ['bottom', 'vertical', false]]) {
  test(`edge docking ${position} creates correctly oriented split`, () => {
    const next = dockPane(pane('a'), pane('b'), 'a', position);
    assert.equal(next.type, 'split');
    assert.equal(next.direction, direction);
    assert.equal(next[incomingFirst ? 'first' : 'second'].id, 'b');
    assert.deepEqual(sortedIds(next), ['a', 'b']);
    assert.equal(validateLayout(next), true);
  });
}

test('moving existing session into another group never duplicates its ID', () => {
  const original = split('s', stack('left', [pane('a'), pane('b')]), stack('right', [pane('c'), pane('d')]));
  const next = dockPane(original, { ...pane('b'), title: 'stale drag title' }, 'right', 'center');
  assert.deepEqual(next.first.panes.map((item) => item.id), ['a']);
  assert.deepEqual(next.second.panes.map((item) => item.id), ['c', 'd', 'b']);
  assert.equal(next.second.activePaneId, 'b');
  assert.equal(findNode(next, 'b').title, 'Session b');
  assert.equal(validateLayout(next), true);
});

test('moving only leaf across sibling handles split pruning without data loss', () => {
  const original = split('s', pane('a'), pane('b'));
  const next = dockPane(original, pane('a'), 'b', 'bottom');
  assert.equal(next.direction, 'vertical');
  assert.equal(next.first.id, 'b');
  assert.equal(next.second.id, 'a');
  assert.deepEqual(sortedIds(next), ['a', 'b']);
  assert.equal(validateLayout(next), true);
});

test('dragging a tab to its own group edge detaches only that tab', () => {
  const original = stack('tabs', [pane('a'), pane('b'), pane('c')]);
  const next = dockPane(original, pane('b'), 'tabs', 'right');
  assert.deepEqual(next.first.panes.map((item) => item.id), ['a', 'c']);
  assert.equal(next.second.id, 'b');
  assert.equal(validateLayout(next), true);
});

test('self drops and malformed or missing destinations are no-ops', () => {
  const a = pane('a');
  assert.equal(dockPane(a, a, 'a', 'right'), a);
  assert.equal(dockPane(a, pane('b'), 'missing', 'center'), a);
  assert.equal(dockPane(a, { type: 'pane', id: 'b', kind: 'script' }, 'a', 'center'), a);
});

test('closing active tab selects neighbor, and never changes session metadata', () => {
  const original = stack('tabs', [pane('a'), pane('b'), pane('c')], 'b');
  const next = closePane(original, 'b');
  assert.equal(next.activePaneId, 'c');
  assert.deepEqual(sortedIds(next), ['a', 'c']);
  assert.deepEqual(next.panes[0].metadata, { session_id: 'a', input: 'draft-a' });
  assert.equal(original.panes.length, 3);
});

test('closing last leaf leaves an addable empty stack; branches are pruned', () => {
  const original = split('s', pane('a'), pane('b'));
  const first = closePane(original, 'a');
  assert.equal(first.id, 'b');
  const empty = closePane(first, 'b');
  assert.equal(empty.type, 'stack');
  assert.equal(empty.panes.length, 0);
  assert.equal(validateLayout(empty), true);
  const reopened = dockPane(empty, pane('new'), empty.id, 'center');
  assert.equal(reopened.activePaneId, 'new');
});

test('an explicit empty split can be closed without touching its sibling', () => {
  const original = split('s', pane('a'), stack('empty', []));
  assert.deepEqual(closePane(original, 'empty'), pane('a'));
});

for (const preset of ['1:1', '2x2', '1:2:1']) {
  test(`${preset} preset preserves all eleven panes, including inactive tabs and drafts`, () => {
    const original = stack('tabs', Array.from({ length: 11 }, (_, index) => pane(`p${index}`)), 'p9');
    const before = JSON.stringify(original);
    const next = applyPreset(original, preset, 'p9');
    assert.deepEqual(sortedIds(next), sortedIds(original));
    for (const item of flattenPanes(next)) assert.deepEqual(item, findNode(original, item.id));
    assert.equal(validateLayout(next), true);
    assert.equal(JSON.stringify(original), before);
    const activeGroup = findNodes(next, (item) => item.type === 'stack' && item.panes.some((tab) => tab.id === 'p9'))[0];
    assert.equal(activeGroup.activePaneId, 'p9');
  });
}

test('grid fills empty slots and uses true 1:2:1 widths', () => {
  const grid = applyPreset(pane('a'), '2x2');
  assert.equal(findNodes(grid, (item) => item.type === 'stack' && item.panes.length === 0).length, 3);
  const columns = applyPreset(stack('tabs', [pane('a'), pane('b'), pane('c')]), '1:2:1');
  assert.equal(columns.ratio, 0.25);
  assert.equal(columns.second.ratio, 2 / 3);
});

test('responsive projection collapses at minimum dimensions without mutating canonical document', () => {
  const original = split('s', pane('a'), pane('b'));
  const before = JSON.stringify(original);
  const narrow = projectLayout(original, 565, 500, { selectedPaneId: 'b' });
  assert.equal(narrow.type, 'stack');
  assert.equal(narrow.collapsedFrom, 's');
  assert.equal(narrow.activePaneId, 'b');
  assert.deepEqual(sortedIds(narrow), ['a', 'b']);
  assert.equal(JSON.stringify(original), before);
  assert.equal(projectLayout(original, 566, 500).type, 'split');
  assert.equal(projectLayout(original, 1200, 179).type, 'stack');
});

test('nested groups collapse independently, remembering selected tabs per group', () => {
  const original = split('outer', pane('a'), split('inner', pane('b'), pane('c')));
  const view = projectLayout(original, 1000, 500, { selectedPaneId: 'a', activeByGroup: { inner: 'c' } });
  assert.equal(view.type, 'split');
  assert.equal(view.first.id, 'a');
  assert.equal(view.second.type, 'stack');
  assert.equal(view.second.activePaneId, 'c');
});

test('selection across breakpoints remembers all ancestor groups', () => {
  const original = split('outer', pane('a'), split('inner', pane('b'), pane('c')));
  const memory = Object.fromEntries(selectionGroups(original, 'c').map((id) => [id, 'c']));
  assert.deepEqual(memory, { inner: 'c', outer: 'c' });
  assert.equal(projectLayout(original, 380, 800, { selectedPaneId: 'c', activeByGroup: memory }).activePaneId, 'c');
  const intermediate = projectLayout(original, 1000, 800, { selectedPaneId: 'a', activeByGroup: memory });
  assert.equal(intermediate.second.activePaneId, 'c');
});

test('responsive tab selection restores the right active tab inside its canonical group', () => {
  const original = split('s', stack('tabs', [pane('a'), pane('b')]), pane('c'));
  const selected = activatePane(original, 'b');
  assert.equal(projectLayout(selected, 320, 600, { selectedPaneId: 'b' }).activePaneId, 'b');
  const wide = projectLayout(selected, 1400, 600);
  assert.equal(wide.first.activePaneId, 'b');
});

test('closing, opening and editing while collapsed survives restoration at wider sizes', () => {
  let canonical = split('s', pane('a'), stack('tabs', [pane('b'), pane('c')], 'c'), 'horizontal', 0.42);
  assert.equal(projectLayout(canonical, 360, 600).type, 'stack');
  canonical = closePane(canonical, 'b');
  canonical = dockPane(canonical, pane('d'), 'tabs', 'center');
  canonical = updatePane(canonical, { ...pane('c'), title: 'Edited title', metadata: { session_id: 'c', input: 'unsaved new text' } });
  canonical = resizeSplit(canonical, 's', 0.62);
  const view = projectLayout(canonical, 360, 600, { selectedPaneId: 'd' });
  assert.deepEqual(sortedIds(view), ['a', 'c', 'd']);
  const wide = projectLayout(canonical, 1800, 900);
  assert.equal(wide.type, 'split');
  assert.equal(wide.ratio, 0.62);
  assert.equal(wide.second.activePaneId, 'd');
  assert.equal(findNode(wide, 'c').metadata.input, 'unsaved new text');
  assert.equal(findNode(wide, 'b'), undefined);
  assert.equal(validateLayout(canonical), true);
});

test('center docking into projected group does not flatten its canonical split', () => {
  const original = split('s', pane('a'), pane('b'));
  const next = dockPane(original, pane('c'), 's', 'center');
  assert.equal(next.type, 'split');
  assert.equal(next.first.type, 'stack');
  assert.equal(next.second.id, 'b');
  assert.deepEqual(sortedIds(next), ['a', 'b', 'c']);
});

test('legacy collapsed snapshot restoration honors closes, edits and new tabs', () => {
  const original = { version: 1, root: split('s', pane('a'), pane('b')) };
  const collapsed = collapseBelowMin(original, 320, 600);
  assert.equal(collapsed.root.type, 'stack');
  collapsed.root.panes = [{ ...pane('b'), title: 'updated' }, pane('c')];
  collapsed.root.activePaneId = 'c';
  const recovered = restoreCollapsed(collapsed);
  assert.deepEqual(sortedIds(recovered.root), ['b', 'c']);
  assert.equal(findNode(recovered.root, 'b').title, 'updated');
  assert.equal(recovered.collapsed, undefined);
  assert.equal(validateLayout(recovered), true);
});

test('validation rejects NaN, duplicate IDs, dangling tabs, bogus kinds and cycles', () => {
  assert.equal(validateLayout(split('s', pane('a'), pane('b'), 'horizontal', NaN)), false);
  assert.equal(validateLayout(split('s', pane('a'), pane('a'))), false);
  assert.equal(validateLayout(stack('tabs', [pane('a')], 'missing')), false);
  assert.equal(isPaneNode({ ...pane('a'), kind: 'stack' }), false);
  const cycle = split('s', pane('a'), pane('b'));
  cycle.second = cycle;
  assert.equal(validateLayout(cycle), false);
});

test('repeated move / preset / close / project operations maintain invariant IDs', () => {
  let root = stack('tabs', Array.from({ length: 18 }, (_, i) => pane(`s${i}`)));
  for (let index = 0; index < 35; index++) {
    root = applyPreset(root, ['1:1', '2x2', '1:2:1'][index % 3]);
    const items = flattenPanes(root);
    root = dockPane(root, items[index % items.length], items[(index + 3) % items.length].id, ['center', 'left', 'bottom'][index % 3]);
    const snapshot = cloneNode(root);
    const projected = projectLayout(root, index % 2 ? 380 : 1700, 800);
    assert.deepEqual(sortedIds(projected), sortedIds(root));
    assert.deepEqual(root, snapshot);
    assert.equal(validateLayout(root), true);
  }
  assert.equal(flattenPanes(root).length, 18);
});
