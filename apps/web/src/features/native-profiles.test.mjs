import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isSameNativeSource, nativeProfileImportPayload, nativeProfileRenamePayload, nativeProfileUpdatePayload, profileEnvironmentPayload, profileEnvironmentDraftChanged, sessionEffortOverride, sessionModelOverride } from './native-profiles.ts';
import { environmentRows } from './environment-model.ts';

const source = { id: 'native-codex', provider: 'codex', label: 'Host Codex', path: '/host/native/config', available: true };
const profile = { id: 'shared-profile', provider: 'codex', native_config: { source_id: source.id, config_dir: source.path } };

test('an existing native source matches provider, source ID and canonical directory together', () => {
  assert.equal(isSameNativeSource(profile, source), true);
  assert.equal(isSameNativeSource(profile, { ...source, label: 'Renamed source' }), true);
  assert.equal(isSameNativeSource(profile, { ...source, path: '/host/different-account' }), false);
  assert.equal(isSameNativeSource(profile, { ...source, provider: 'claude_code' }), false);
  assert.equal(isSameNativeSource(profile, { ...source, id: 'different-source' }), false);
  assert.equal(isSameNativeSource({ ...profile, native_config: null }, source), false);
  assert.equal(isSameNativeSource(profile, undefined), false);
});

test('native sources preserve unset versus explicit directory environment for authentication context', () => {
  assert.equal(isSameNativeSource(profile, { ...source, config_env: null }), true);
  assert.equal(isSameNativeSource({ ...profile, native_config: { ...profile.native_config, config_env: null } }, source), true);
  assert.equal(isSameNativeSource(profile, { ...source, config_env: source.path }), false);
  const explicitProfile = { ...profile, native_config: { ...profile.native_config, config_env: source.path } };
  assert.equal(isSameNativeSource(explicitProfile, source), false);
  assert.equal(isSameNativeSource(explicitProfile, { ...source, config_env: source.path }), true);
  assert.equal(isSameNativeSource(explicitProfile, { ...source, config_env: '/different-spelling-of-same-directory' }), false);
});

test('native import requires an available source and explicit shared-configuration consent', () => {
  assert.equal(nativeProfileImportPayload(source, 'Personal', false), undefined);
  assert.equal(nativeProfileImportPayload({ ...source, available: false }, 'Personal', true), undefined);
  assert.equal(nativeProfileImportPayload(undefined, 'Personal', true), undefined);
});

test('native import posts only a source reference, optional name and consent, never host credentials or overrides', () => {
  assert.deepEqual(nativeProfileImportPayload(source, ' Personal Codex ', true), { source_id: 'native-codex', name: 'Personal Codex', confirmed_shared_config: true });
  assert.deepEqual(nativeProfileImportPayload(source, '  ', true), { source_id: 'native-codex', confirmed_shared_config: true });
});

test('renaming a native profile sends no mutable client settings', () => {
  assert.deepEqual(nativeProfileRenamePayload('  Host work account '), { name: 'Host work account' });
});

test('native environment edits send only name and explicit overrides, including an empty replacement', () => {
  const environment = { MODE: { kind: 'literal', value: 'new' }, OPENAI_API_KEY: { kind: 'secret_ref', reference: 'env:AGENTDOCK_SECRET_FIXTURE' } };
  const initial = environmentRows({ MODE: { kind: 'literal', value: 'old' } });
  assert.deepEqual(nativeProfileUpdatePayload(' Shared ', environmentRows(environment), true, initial), { name: 'Shared', environment });
  assert.deepEqual(nativeProfileUpdatePayload(' Shared ', [], true, initial), { name: 'Shared', environment: {} });
  assert.deepEqual(profileEnvironmentPayload([], true), { environment: {} });
});

test('unsupported backends never silently discard nonempty, invalid or cleared environment edits', () => {
  const initial = environmentRows({ MODE: { kind: 'literal', value: 'old' } });
  assert.deepEqual(nativeProfileUpdatePayload('Rename only', initial, false, initial), { name: 'Rename only' });
  assert.deepEqual(profileEnvironmentPayload([], false), {});
  assert.throws(() => profileEnvironmentPayload(environmentRows({ MODE: { kind: 'literal', value: 'new' } }), false, initial), /Upgrade/);
  assert.throws(() => profileEnvironmentPayload([], false, initial), /Upgrade/);
  assert.throws(() => profileEnvironmentPayload([{ id: 'draft', name: '', kind: 'literal', value: 'keep draft' }], false), /Upgrade/);
  assert.throws(() => profileEnvironmentPayload(environmentRows({ HOME: { kind: 'literal', value: '/forbidden' } }), true), /cannot be overridden/);
  assert.equal(profileEnvironmentDraftChanged(initial.map(row => ({ ...row, id: 'different-ui-row' })), initial), false);
});

test('shared native sessions ignore stale model overrides rather than changing the original configuration', () => {
  assert.deepEqual(sessionModelOverride('codex', profile, 'stale-custom-model'), {});
  assert.deepEqual(sessionModelOverride('claude_code', { ...profile, provider: 'claude_code' }, 'sonnet'), {});
});

test('custom and isolated sessions retain model selection; terminals never get a model override', () => {
  assert.deepEqual(sessionModelOverride('codex', undefined, '  custom-model  '), { model: 'custom-model' });
  assert.deepEqual(sessionModelOverride('claude_code', { native_config: null }, 'fast'), { model: 'fast' });
  assert.deepEqual(sessionModelOverride('codex', undefined, '  '), {});
  assert.deepEqual(sessionModelOverride('terminal', undefined, 'custom-model'), {});
});

test('session effort overrides stay with the selected non-native client configuration', () => {
  assert.deepEqual(sessionEffortOverride('codex', undefined, ' high '), { effort: 'high' });
  assert.deepEqual(sessionEffortOverride('claude_code', { native_config: null }, 'low'), { effort: 'low' });
  assert.deepEqual(sessionEffortOverride('codex', profile, 'high'), {});
  assert.deepEqual(sessionEffortOverride('terminal', undefined, 'high'), {});
  assert.deepEqual(sessionEffortOverride('codex', undefined, '  '), {});
});
