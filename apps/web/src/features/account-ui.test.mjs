import { test } from 'node:test';
import assert from 'node:assert/strict';
import { accountLimitPercent, accountLoginUrl, accountResetDate, accountStatusLabel, accountResetCreditState, canResetAccountQuota, createQuotaResetStore, quotaResetOutcomeNotice, QUOTA_RESET_STORAGE_KEY, QUOTA_RESET_STORAGE_WARNING } from './account-ui.ts';

const fixture = (changes = {}) => ({ id: 'account-one', name: 'Fixture account', provider: 'codex', profile_id: 'profile-one', storage_path: '/fixture/account', status: 'signed_in', limits: { reset_credits: 2 }, capabilities: { login: true, quota: true, reset_quota: true, refresh_token: true, logout: true }, ...changes });

test('official account status does not invent a successful login from unknown or error state', () => {
  assert.equal(accountStatusLabel('unknown'), 'Not checked'); assert.equal(accountStatusLabel('error'), 'Account needs attention'); assert.equal(accountStatusLabel('signed_out'), 'Signed out');
});
test('unknown and invalid quota values are not displayed as zero percent', () => {
  assert.equal(accountLimitPercent(), undefined);
  for (const used_percent of [NaN, Infinity, -1, 101]) assert.equal(accountLimitPercent({ used_percent }), undefined);
  assert.equal(accountLimitPercent({ used_percent: 0 }), 0); assert.equal(accountLimitPercent({ used_percent: 75.5 }), 75.5);
});
test('window mappings require an exact official duration and never guess from primary or secondary alone', () => {
  const known = { used_percent: 12, window_minutes: 300, window_kind: 'five_hour', mapping_basis: 'duration_minutes' };
  assert.equal(known.window_kind, 'five_hour');
  assert.equal(accountLimitPercent({ used_percent: 12, window_minutes: 299 }), 12);
});
test('reset-credit state distinguishes official availability from unsupported or unreported data', () => {
  assert.equal(accountResetCreditState(fixture({ limits: { reset_credits: 2, reset_credits_source: 'official' } })), 'available');
  assert.equal(accountResetCreditState(fixture({ limits: { reset_credits: 0, reset_credits_source: 'official' } })), 'empty');
  assert.equal(accountResetCreditState(fixture({ limits: { reset_credits: 2 } })), 'not_reported');
  assert.equal(accountResetCreditState(fixture({ status: 'signed_out' })), 'signed_out');
  assert.equal(accountResetCreditState(fixture({ capabilities: { ...fixture().capabilities, quota: false } })), 'unsupported');
});

test('official no-credit, nothing-to-reset and already-redeemed outcomes are never described as a new reset success', () => {
  assert.equal(quotaResetOutcomeNotice('reset'), 'Official quota reset completed.');
  for (const outcome of ['alreadyRedeemed', 'nothingToReset', 'noCredit']) assert.notEqual(quotaResetOutcomeNotice(outcome), 'Official quota reset completed.');
  assert.match(quotaResetOutcomeNotice('nothingToReset'), /No quota reset was performed/);
  assert.match(quotaResetOutcomeNotice('noCredit'), /No quota reset was performed/);
});
test('official reset times support epoch seconds and ISO strings without inventing missing dates', () => {
  assert.equal(accountResetDate(), undefined); assert.equal(accountResetDate('not-a-date'), undefined);
  assert.equal(accountResetDate(1_800_000_000).getTime(), 1_800_000_000_000);
  assert.equal(accountResetDate('2026-09-09T12:00:00Z').toISOString(), '2026-09-09T12:00:00.000Z');
});
test('sign-in links are explicit official HTTPS links with no userinfo or executable scheme', () => {
  for (const url of ['https://auth.openai.com/device', 'https://chatgpt.com/auth/login']) assert.equal(accountLoginUrl(fixture({ login: { url } })), url);
  for (const url of ['javascript:alert(1)', 'http://auth.openai.com/login', 'https://auth.openai.com.evil.invalid/login', 'https://user:pass@auth.openai.com/login', 'https://claude.ai/login', 'https://evil.invalid']) assert.equal(accountLoginUrl(fixture({ login: { url } })), undefined);
  assert.equal(accountLoginUrl(fixture({ provider: 'claude_code', login: { url: 'https://claude.ai/login' } })), 'https://claude.ai/login');
});
test('quota reset requires an official capability, signed-in status and a real available credit', () => {
  assert.equal(canResetAccountQuota(fixture()), true);
  for (const changes of [{ status: 'unknown' }, { status: 'signed_out' }, { limits: undefined }, { limits: { reset_credits: 0 } }, { limits: { reset_credits: NaN } }, { capabilities: { ...fixture().capabilities, reset_quota: false } }, { capabilities: { ...fixture().capabilities, quota: false } }]) assert.equal(canResetAccountQuota(fixture(changes)), false);
});
test('uncertain quota reset retries preserve the original request key, credit and account scope', () => {
  let generated = 0; const store = createQuotaResetStore(() => 'request-' + ++generated), account = fixture();
  assert.throws(() => store.begin(account, false), /Confirm/); assert.equal(generated, 0);
  const first = store.begin(account, true, 'credit-one'); store.unknown(account.id);
  assert.equal(store.get(account.id).outcome, 'unknown');
  const retry = store.begin(fixture({ limits: { reset_credits: 0 } }), true, 'different-credit');
  assert.equal(retry, first); assert.equal(retry.idempotency_key, 'request-1'); assert.equal(retry.credit_id, 'credit-one'); assert.equal(generated, 1);
  const other = store.begin(fixture({ id: 'account-two' }), true); assert.equal(other.idempotency_key, 'request-2');
  store.complete(account.id); assert.equal(store.get(account.id), undefined); assert.equal(store.get('account-two'), other);
});
test('unsupported official quota reset never creates a request key or mocks a successful action', () => {
  let generated = 0; const store = createQuotaResetStore(() => String(++generated));
  assert.throws(() => store.begin(fixture({ capabilities: { ...fixture().capabilities, reset_quota: false } }), true), /does not currently offer/);
  assert.equal(generated, 0); assert.equal(store.get('account-one'), undefined);
});

const memoryStorage = () => { const data = new Map(); return { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) }; };
test('pending reset key survives a full store recreation without persisting private account metadata', () => {
  const storage = memoryStorage(), account = fixture({ email: 'private@example.invalid', login: { url: 'https://auth.openai.com/private-login-link' } });
  const one = createQuotaResetStore(() => 'original-idempotency-key', storage, true);
  one.begin(account, true); one.unknown(account.id);
  assert.deepEqual(JSON.parse(storage.getItem(QUOTA_RESET_STORAGE_KEY)), [{ account_id: account.id, idempotency_key: 'original-idempotency-key' }]);
  let generated = 0; const reloaded = createQuotaResetStore(() => String(++generated), storage, true);
  assert.equal(reloaded.get(account.id).outcome, 'unknown');
  assert.equal(reloaded.begin(fixture({ limits: { reset_credits: 0 } }), true).idempotency_key, 'original-idempotency-key'); assert.equal(generated, 0);
  assert.equal(reloaded.complete(account.id), true); assert.deepEqual(JSON.parse(storage.getItem(QUOTA_RESET_STORAGE_KEY)), []);
});
test('unavailable, denied, read-only or corrupt reset storage blocks new request keys before any resource call', () => {
  for (const storage of [undefined, { getItem() { throw Error('denied'); }, setItem() {} }, { getItem() { return null; }, setItem() { throw Error('quota'); } }, { getItem() { return '{invalid'; }, setItem() {} }]) {
    let generated = 0; const store = createQuotaResetStore(() => String(++generated), storage, true);
    assert.throws(() => store.begin(fixture(), true), { message: QUOTA_RESET_STORAGE_WARNING });
    assert.equal(generated, 0); assert.equal(store.storageAvailable(), false);
  }
});
test('a failed cleanup keeps the previous request key instead of allowing another new credit request', () => {
  const backing = memoryStorage(); let denyWrites = false;
  const storage = { getItem: backing.getItem, setItem(key, value) { if (denyWrites) throw Error('denied'); backing.setItem(key, value); } };
  let generated = 0; const store = createQuotaResetStore(() => 'id-' + ++generated, storage, true);
  store.begin(fixture(), true); denyWrites = true;
  assert.equal(store.complete('account-one'), false); assert.equal(store.get('account-one').idempotency_key, 'id-1');
  assert.equal(store.begin(fixture(), true).idempotency_key, 'id-1'); assert.equal(generated, 1);
});
