import { after, afterEach, before, mock, test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { createServer } from 'vite';
import vue from '@vitejs/plugin-vue';
import { createSSRApp } from 'vue';
import { renderToString } from 'vue/server-renderer';

let server, Dialog, capabilities, rows;
const profile = (native = false) => ({ id: 'profile-fixture', name: 'Fixture', provider: 'codex', endpoint_url: null, model: null, permission_mode: 'native', secret_ref: null, proxy_url: null, model_aliases: {}, environment: { MODE: { kind: 'literal', value: 'old' } }, native_config: native ? { source_id: 'codex-fixture', config_dir: '/fixture/native', config_env: null } : null, created_at: '2026-01-01T00:00:00Z' });
before(async () => {
  server = await createServer({ configFile: false, root: fileURLToPath(new URL('../../', import.meta.url)), plugins: [vue()], server: { middlewareMode: true, hmr: false, ws: false }, appType: 'custom', optimizeDeps: { noDiscovery: true } });
  Dialog = (await server.ssrLoadModule('/src/features/ProfilesDialog.vue')).default;
  capabilities = (await server.ssrLoadModule('/src/features/backend-capabilities.ts')).backendCapabilities;
  rows = (await server.ssrLoadModule('/src/features/environment-model.ts')).environmentRows;
});
after(async () => { await server?.close(); });
afterEach(() => mock.restoreAll());

async function setup(profiles = []) {
  let state;
  const Harness = { ...Dialog, setup(props, context) { state = Dialog.setup(props, context); return state; } };
  const context = {};
  const html = await renderToString(createSSRApp(Harness, { profiles }), context);
  return { state, html: html + Object.values(context.teleports ?? {}).join('') };
}

test('actual profiles SFC embeds a capability-gated environment editor', async () => {
  capabilities.environment = false;
  let { html } = await setup();
  assert.ok(html.includes('class="environment-editor"'));
  let section = html.slice(html.indexOf('class="environment-editor"'));
  assert.match(section, /class="small-button"[^>]*disabled/);
  capabilities.environment = true;
  ({ html } = await setup());
  section = html.slice(html.indexOf('class="environment-editor"'));
  assert.doesNotMatch(section, /class="small-button"[^>]*disabled/);
});

test('actual native profile save initializes its environment and sends an explicit empty replacement only', async () => {
  capabilities.environment = true;
  const original = profile(true);
  const { state } = await setup([original]);
  state.edit(original);
  assert.equal(state.environmentDraft.value[0].value, 'old');
  state.environmentDraft.value = [];
  let sent;
  mock.method(globalThis, 'fetch', async (url, init) => {
    sent = { url, method: init.method, body: JSON.parse(init.body) };
    return new Response(JSON.stringify({ ...original, ...sent.body }), { status: 200 });
  });
  await state.save();
  assert.deepEqual(sent, { url: '/api/endpoint-profiles/profile-fixture', method: 'PATCH', body: { name: 'Fixture', environment: {} } });
});

test('custom profile save includes overrides while model discovery never receives environment drafts', async () => {
  capabilities.environment = true; capabilities.models = true;
  const original = profile();
  const { state } = await setup([original]);
  state.edit(original);
  state.environmentDraft.value = rows({ MODE: { kind: 'literal', value: 'new' } });
  const calls = [];
  mock.method(globalThis, 'fetch', async (url, init) => {
    const body = JSON.parse(init.body); calls.push({ url, body });
    return new Response(JSON.stringify(url.endsWith('discover-models') ? { models: [], source_url: 'https://example.invalid/models', has_more: false } : { ...original, ...body }), { status: 200 });
  });
  await state.discover();
  assert.equal(calls[0].url, '/api/endpoint-profiles/discover-models');
  assert.equal(Object.hasOwn(calls[0].body, 'environment'), false);
  await state.save();
  assert.deepEqual(calls[1].body.environment, { MODE: { kind: 'literal', value: 'new' } });
});

test('capability loss blocks save and navigation asks before discarding raw environment drafts', async () => {
  capabilities.environment = true;
  const original = profile();
  const { state } = await setup([original]);
  state.edit(original);
  state.environmentDraft.value = [{ id: 'incomplete', name: '', kind: 'literal', value: 'unsaved draft' }];
  capabilities.environment = false;
  const fetch = mock.method(globalThis, 'fetch', async () => { throw new Error('must not send a request'); });
  await state.save();
  assert.equal(fetch.mock.callCount(), 0);
  assert.match(state.environmentError.value, /Upgrade/);
  let left = false;
  state.requestLeave(() => { left = true; });
  assert.equal(left, false);
  assert.equal(state.environmentDraft.value[0].value, 'unsaved draft');
  state.discardEnvironmentDraft();
  assert.equal(left, true);
  assert.equal(state.environmentDraft.value[0].value, 'old');
});
