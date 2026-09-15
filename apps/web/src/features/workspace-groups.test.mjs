import { test } from 'node:test';
import assert from 'node:assert/strict';
import { emptyWorkspaceGroupPreferences, groupWorkspaces, loadWorkspaceGroupPreferences, parseWorkspaceGroupPreferences, saveWorkspaceGroupPreferences, workspaceGroupStorageKey, workspaceSessionPane } from './workspace-groups.ts';

const workspace = (id, name = id) => ({ id, name, root_path: `/projects/${id}`, created_at: 'now' });
const session = (id, workspace_id, title = id, status = 'stopped', provider = 'codex') => ({ id, workspace_id, title, status, provider, created_at: 'now', updated_at: 'now', endpoint_profile_id: null, provider_session_id: null, error: null });
const workspaces = [workspace('alpha', 'Frontend'), workspace('beta', '后端服务'), workspace('empty')];
const sessions = [session('a1', 'alpha', 'Refactor payment', 'running'), session('a2', 'alpha', 'Fix toolbar'), session('b1', 'beta', '修复实名认证流程', 'waiting', 'claude_code')];

test('workspaces own exactly their sessions and retain empty workspaces without inventing orphan groups', () => {
  const groups = groupWorkspaces(workspaces, [...sessions, session('orphan', 'unknown'), sessions[0]]);
  assert.deepEqual(groups.map(group => [group.workspace.id, group.sessions.map(item => item.id), group.activeCount]), [['alpha', ['a1', 'a2'], 1], ['beta', ['b1'], 1], ['empty', [], 0]]);
});

test('archived sessions stay out of ordinary workspace groups and active counts until restored', () => {
  const archived = { ...sessions[0], archived_at: '2026-09-13T00:00:00Z' };
  const grouped = groupWorkspaces(workspaces, [archived, ...sessions]);
  assert.deepEqual(grouped[0].sessions.map(item => item.id), ['a2']);
  assert.equal(grouped[0].activeCount, 0);
  assert.equal(grouped[0].totalSessions, 1);
  assert.deepEqual(groupWorkspaces(workspaces, [archived], { query: 'payment' }), []);
  const restored = groupWorkspaces(workspaces, [{ ...archived, archived_at: null }]);
  assert.equal(restored[0].sessions[0].id, archived.id);
  assert.equal(restored[0].activeCount, 1);
  assert.equal(archived.status, 'running');
});

test('selected and active workspaces expand by default while explicit collapse survives polling', () => {
  const preferences = emptyWorkspaceGroupPreferences();
  assert.deepEqual(groupWorkspaces(workspaces, sessions, { selectedWorkspaceId: 'empty' }).map(group => group.expanded), [true, true, true]);
  preferences.expanded.alpha = false;
  const options = { selectedWorkspaceId: 'alpha', preferences };
  assert.equal(groupWorkspaces(workspaces, sessions, options)[0].expanded, false);
  assert.equal(groupWorkspaces(workspaces, [...sessions, session('a3', 'alpha', 'Another run', 'starting')], options)[0].expanded, false);
  assert.equal(groupWorkspaces(workspaces, [], options)[0].expanded, false);
});

test('explicit expansion is retained after active sessions finish', () => {
  const preferences = emptyWorkspaceGroupPreferences();preferences.expanded.beta = true;
  assert.equal(groupWorkspaces(workspaces, sessions.map(item => ({ ...item, status: 'stopped' })), { preferences })[1].expanded, true);
});

test('workspace-name search reveals its sessions; session search retains only matching children', () => {
  assert.deepEqual(groupWorkspaces(workspaces, sessions, { query: 'frnt' }).map(group => group.sessions.map(item => item.id)), [['a1', 'a2']]);
  const groups = groupWorkspaces(workspaces, sessions, { query: 'tlbr' });
  assert.deepEqual(groups.map(group => [group.workspace.id, group.sessions.map(item => item.id), group.totalSessions]), [['alpha', ['a2'], 2]]);
});

test('Chinese and NFKC/fullwidth provider searches remain local fuzzy matches', () => {
  assert.deepEqual(groupWorkspaces(workspaces, sessions, { query: '认证' }).map(group => group.workspace.id), ['beta']);
  assert.deepEqual(groupWorkspaces(workspaces, sessions, { query: 'ＣＯＤＥＸ' }).flatMap(group => group.sessions.map(item => item.id)).sort(), ['a1', 'a2']);
  assert.deepEqual(groupWorkspaces(workspaces, sessions, { query: 'frontend toolbar' }).flatMap(group => group.sessions.map(item => item.id)), ['a2']);
  assert.deepEqual(groupWorkspaces(workspaces, sessions, { query: 'does not exist' }), []);
});

test('search expansion and manual search collapse are temporary and do not overwrite normal choices', () => {
  const preferences = emptyWorkspaceGroupPreferences();preferences.expanded.alpha = false;
  assert.equal(groupWorkspaces(workspaces, sessions, { query: 'payment', preferences })[0].expanded, true);
  assert.equal(groupWorkspaces(workspaces, sessions, { query: 'payment', preferences, searchExpanded: { alpha: false } })[0].expanded, false);
  assert.equal(groupWorkspaces(workspaces, sessions, { query: '', preferences })[0].expanded, false);
  assert.equal(preferences.expanded.alpha, false);
});

test('pins preserve explicit order ahead of ordinary workspaces without moving sessions across groups', () => {
  const preferences = emptyWorkspaceGroupPreferences();preferences.pinned = ['empty', 'beta'];
  const groups = groupWorkspaces(workspaces, sessions, { preferences });
  assert.deepEqual(groups.map(group => group.workspace.id), ['empty', 'beta', 'alpha']);
  assert.deepEqual(groups[1].sessions.map(item => item.id), ['b1']);
  assert.deepEqual(workspaces.map(item => item.id), ['alpha', 'beta', 'empty']);
});

test('session drag payload IDs are globally stable and include workspace ownership', () => {
  assert.deepEqual(workspaceSessionPane(sessions[0]), { type: 'pane', id: 'session-a1', kind: 'agent_chat', title: 'Refactor payment', metadata: { workspace_id: 'alpha', session_id: 'a1', provider: 'codex' } });
  assert.equal(workspaceSessionPane(session('shell', 'beta', 'Shell', 'running', 'terminal')).kind, 'terminal');
  assert.notEqual(workspaceSessionPane(sessions[0]).id, workspaceSessionPane(sessions[2]).id);
});

test('preferences validate version, IDs and booleans instead of trusting browser storage', () => {
  const preferences = parseWorkspaceGroupPreferences({ version: 1, pinned: ['alpha', 'alpha', null, '', 'beta'], expanded: { alpha: false, beta: true, empty: 'true' } });
  assert.deepEqual(preferences.pinned, ['alpha', 'beta']);
  assert.deepEqual(Object.entries(preferences.expanded), [['alpha', false], ['beta', true]]);
  assert.deepEqual(parseWorkspaceGroupPreferences({ version: 99, pinned: ['alpha'] }).pinned, []);
  assert.equal(Object.getPrototypeOf(preferences.expanded), null);
});

test('pinned/expanded choices persist by deployment key without storing session text or host paths', () => {
  const writes = new Map();const storage = { getItem: key => writes.get(key), setItem: (key, value) => writes.set(key, value) };
  const key = workspaceGroupStorageKey('workspace-groups-test-host-one');
  const preferences = emptyWorkspaceGroupPreferences();preferences.pinned = ['alpha'];preferences.expanded.beta = false;
  saveWorkspaceGroupPreferences(key, preferences, storage);
  assert.deepEqual(loadWorkspaceGroupPreferences(key, storage), preferences);
  preferences.pinned.push('later-edit');
  assert.deepEqual(loadWorkspaceGroupPreferences(key, storage).pinned, ['alpha']);
  const encoded = writes.get(key);assert.equal(encoded.includes('/projects/'), false);assert.equal(encoded.includes('Refactor'), false);
  assert.deepEqual(loadWorkspaceGroupPreferences(workspaceGroupStorageKey('workspace-groups-test-host-two'), storage).pinned, []);
});

test('unavailable and malformed browser storage falls back to memory across component remounts', () => {
  const key = workspaceGroupStorageKey('workspace-groups-test-denied');
  const storage = { getItem() { throw Error('denied'); }, setItem() { throw Error('quota'); } };
  const preferences = loadWorkspaceGroupPreferences(key, storage);preferences.expanded.alpha = false;
  assert.doesNotThrow(() => saveWorkspaceGroupPreferences(key, preferences, storage));
  assert.equal(loadWorkspaceGroupPreferences(key, storage).expanded.alpha, false);
  const broken = loadWorkspaceGroupPreferences(workspaceGroupStorageKey('workspace-groups-test-corrupt'), { getItem: () => '{broken', setItem() {} });
  assert.deepEqual(broken.pinned, []);
});
