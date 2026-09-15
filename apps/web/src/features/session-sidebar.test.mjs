import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';
import vue from '@vitejs/plugin-vue';
import { createSSRApp } from 'vue';
import { renderToString } from 'vue/server-renderer';

let server, Sidebar, SessionRow, i18n;
const workspace = { id: 'sidebar-workspace', name: 'Fixture workspace', root_path: '/fixture/sidebar', created_at: '2026-09-01T00:00:00Z' };
const session = (id, changes = {}) => ({
  id, workspace_id: workspace.id, title: `Fixture ${id}`, provider: 'codex', status: 'stopped',
  endpoint_profile_id: null, provider_session_id: null, error: null,
  created_at: '2026-09-01T00:00:00Z', updated_at: '2026-09-01T00:00:00Z', ...changes,
});
const current = session('current');
const archived = session('archived', { archived_at: '2026-09-12T00:00:00Z', status: 'running' });
let renderId = 0;

before(async () => {
  server = await createServer({ configFile: false, root: fileURLToPath(new URL('../../', import.meta.url)), plugins: [vue()], server: { middlewareMode: true, hmr: false, ws: false }, appType: 'custom', optimizeDeps: { noDiscovery: true } });
  Sidebar = (await server.ssrLoadModule('/src/features/WorkspaceSidebar.vue')).default;
  SessionRow = (await server.ssrLoadModule('/src/features/SidebarSessionRow.vue')).default;
  i18n = (await server.ssrLoadModule('/src/i18n/index.ts')).useI18n();
  i18n.setLocale('en');
});
after(async () => { await server?.close(); });

async function render(component, props, configure) {
  let state, exposed;
  const wrapper = {
    ...component,
    async setup(props, context) {
      state = component.setup(props, { ...context, expose(value) { exposed = value; context.expose(value); } });
      await configure?.(state);
      return state;
    },
  };
  const html = await renderToString(createSSRApp(wrapper, props));
  return { html, state, exposed };
}
const sidebarProps = changes => ({ workspaces: [workspace], sessions: [current, archived], selectedWorkspaceId: workspace.id, archiveSupported: true, storageKey: `session-sidebar-test-${++renderId}`, ...changes });

test('All sessions expands an actual inline list without emitting the old modal event or fetching', async t => {
  let requests = 0, modalEvents = 0;
  t.mock.method(globalThis, 'fetch', async () => { requests++; throw Error('unexpected request'); });
  const { html, state } = await render(Sidebar, sidebarProps({ onAllSessions: () => modalEvents++ }), bindings => { bindings.toggleAllSessions(); });
  assert.equal(state.allSessionsExpanded.value, true);
  assert.match(html, /id="sidebar-session-library"/);
  assert.match(html, /class="all-sessions-list"/);
  assert.match(html, /aria-expanded="true"[^>]*aria-controls="sidebar-session-library"/);
  assert.doesNotMatch(html, /role="dialog"|data-sidebar-session-id="archived"/);
  assert.equal(modalEvents, 0);
  assert.equal(requests, 0);
  state.toggleAllSessions();
  assert.equal(state.allSessionsExpanded.value, false);
});

test('the rendered list supports archive filters and fuzzy query without duplicating archived workspace rows', async () => {
  const { html } = await render(Sidebar, sidebarProps(), state => {
    state.allSessionsExpanded.value = true;
    state.sessionFiltersExpanded.value = true;
    state.sessionArchive.value = 'archived';
    state.sessionQuery.value = 'fxtr arcvd';
  });
  assert.equal((html.match(/data-sidebar-session-id="archived"/g) ?? []).length, 1);
  assert.equal((html.match(/class="session-library-filters"/g) ?? []).length, 1);
  for (const label of ['workspace', 'provider', 'status', 'archive state']) assert.ok(html.includes(`Filter sessions by ${label}`));
  const ordinary = html.slice(html.indexOf('class="workspace-groups"'));
  assert.doesNotMatch(ordinary, /data-sidebar-session-id="archived"/);
});

test('archive and restore actions emit only reversible metadata intent and reject unsupported or busy attempts', async t => {
  let requests = 0;
  t.mock.method(globalThis, 'fetch', async () => { requests++; throw Error('unexpected request'); });
  for (const fixture of [current, archived]) {
    const changes = [];
    const { html, state } = await render(SessionRow, { session: fixture, archiveSupported: true, onArchive: (...args) => changes.push(args) }, state => state.toggleMenu());
    assert.ok(html.includes(fixture.archived_at ? 'Restore session' : 'Archive session'));
    state.archiveSession();
    assert.deepEqual(changes, [[fixture, !fixture.archived_at]]);
    assert.equal(state.menuOpen.value, false);
  }
  for (const props of [{ archiveSupported: false }, { archiveSupported: true, archiveBusy: true }]) {
    const changes = [];
    const { html, state } = await render(SessionRow, { session: current, ...props, onArchive: (...args) => changes.push(args) }, state => state.toggleMenu());
    assert.match(html, /<button[^>]*class="session-archive-action"[^>]*\bdisabled\b/);
    state.archiveSession();
    assert.deepEqual(changes, []);
  }
  assert.equal(requests, 0);
});

test('sidebar guards duplicate or unsupported archive intent as well as row controls', async () => {
  for (const props of [{ archiveSupported: false }, { archiveBusyIds: [current.id] }]) {
    const events = [];
    const { state } = await render(Sidebar, sidebarProps({ ...props, onArchiveSession: (...args) => events.push(args) }));
    state.archiveSession(current, true);
    assert.deepEqual(events, []);
  }
  const events = [];
  const { state } = await render(Sidebar, sidebarProps({ onArchiveSession: (...args) => events.push(args) }));
  state.archiveSession(current, true);
  assert.deepEqual(events, [[current, true]]);
});

test('session environment and Escape stay accessible from the row menu without opening a client', async () => {
  const environment = [], opened = [];
  const { html, state } = await render(SessionRow, { session: current, archiveSupported: true, onEnvironment: id => environment.push(id), onOpen: value => opened.push(value) }, state => state.toggleMenu());
  assert.ok(html.includes('Session actions for Fixture current'));
  // Locating in the canvas moved into this menu so it costs the row no width.
  assert.ok(html.includes('Locate in canvas'));
  assert.ok(html.includes('Session environment'));
  state.environment();
  assert.deepEqual(environment, [current.id]);
  assert.deepEqual(opened, []);
  let focused = 0, prevented = 0, stopped = 0;
  state.menuButton.value = { focus() { focused++; } };
  state.toggleMenu();
  state.escapeMenu({ key: 'Escape', preventDefault() { prevented++; }, stopPropagation() { stopped++; } });
  await Promise.resolve();
  assert.equal(state.menuOpen.value, false);
  assert.equal(prevented, 1); assert.equal(stopped, 1); assert.ok(focused >= 1);
});

test('session rows expose a rename action without opening or changing the session', async () => {
  const renamed = [];
  const { html, state } = await render(SessionRow, { session: current, archiveSupported: true, onRename: (...args) => renamed.push(args) }, state => state.toggleMenu());
  assert.ok(html.includes('Rename session'));
  state.beginRename();
  assert.equal(state.renaming.value, true); assert.equal(state.titleDraft.value, current.title);
  state.titleDraft.value = '  Concrete Claude task  '; state.submitRename();
  assert.equal(state.renaming.value, false);
  assert.deepEqual(renamed, [[current, 'Concrete Claude task']]);
});

test('revealSession expands and scrolls the matching row while preserving canvas selection and CLI state', async () => {
  const events = [], scrolls = [], focuses = [];
  const { state, exposed } = await render(Sidebar, sidebarProps({ onSelectWorkspace: id => events.push(id), onOpenSession: value => events.push(value) }));
  state.query.value = 'nonmatching search';
  state.preferences.value.expanded[workspace.id] = false;
  state.sidebarElement.value = { querySelector(selector) {
    assert.equal(selector, '.workspace-groups');
    return { querySelectorAll() { return [{ dataset: { sidebarSessionId: current.id }, scrollIntoView(options) { scrolls.push(options); }, querySelector() { return { focus(options) { focuses.push(options); } }; } }]; } };
  } };
  assert.equal(await exposed.revealSession(current.id), true);
  assert.equal(state.query.value, '');
  assert.equal(state.preferences.value.expanded[workspace.id], true);
  assert.equal(scrolls.length, 1); assert.deepEqual(focuses, [{ preventScroll: true }]);
  assert.deepEqual(events, []);
  assert.equal(await exposed.revealSession('missing'), false);
});

test('revealSession finds an archived row by opening archived inline results and clearing all blockers', async () => {
  const { state, exposed } = await render(Sidebar, sidebarProps());
  state.sessionQuery.value = 'absent'; state.sessionProvider.value = 'terminal'; state.sessionWorkspace.value = 'absent'; state.sessionStatus.value = 'failed';
  assert.equal(await exposed.revealSession(archived.id), true);
  assert.equal(state.allSessionsExpanded.value, true);
  assert.equal(state.sessionArchive.value, 'archived');
  assert.deepEqual(state.filteredSessions.value.map(entry => entry.session.id), [archived.id]);
});

test('bilingual row actions translate application text but preserve session titles', async () => {
  const title = 'Keep original 标题';
  for (const [locale, label] of [['zh-CN', '归档会话'], ['en', 'Archive session']]) {
    i18n.setLocale(locale);
    const { html } = await render(SessionRow, { session: { ...current, title }, archiveSupported: true }, state => state.toggleMenu());
    assert.ok(html.includes(label)); assert.ok(html.includes(title));
  }
  i18n.setLocale('en');
});
