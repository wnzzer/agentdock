import type { AgentProviderKind } from '@agentdock/protocol';

export type AccountWindowKind = 'five_hour' | 'weekly';
export type AccountWindowMappingBasis = 'duration_minutes' | 'status_line_window' | 'oauth_usage';
export interface AccountLimit {
  used_percent: number;
  window_minutes?: number;
  resets_at?: string | number;
  /** Present only when the native client reported an exact known duration. */
  window_kind?: AccountWindowKind;
  mapping_basis?: AccountWindowMappingBasis;
}
export type AccountUsageAvailability = 'available' | 'stale' | 'not_reported' | 'capture_missing' | 'capture_disabled' | 'credential_unavailable' | 'query_failed' | 'rate_limited' | 'unauthorized' | 'cached';
/** Explicit Claude usage snapshot; source identifies OAuth or status-line data. */
export interface AccountUsage {
  availability: AccountUsageAvailability;
  capture_path?: string;
  captured_at?: number;
  source?: 'claude_oauth_usage' | 'status_line_capture';
  /** Provider-supplied wait hint for a rate-limited query. */
  retry_after_seconds?: number;
  /** Set when a failed query kept the previous figures; stamps when they were read. */
  retained_checked_at?: string;
}
/** Human wait hint for a rate-limited usage query, in whole minutes or seconds. */
export function usageRetryHint(seconds?: number): { unit: 'seconds' | 'minutes'; value: number } | undefined {
  if (seconds === undefined || !Number.isFinite(seconds) || seconds <= 0) return undefined;
  return seconds < 90 ? { unit: 'seconds', value: Math.ceil(seconds) } : { unit: 'minutes', value: Math.ceil(seconds / 60) };
}
export type AccountResetOutcome = 'reset' | 'alreadyRedeemed' | 'nothingToReset' | 'noCredit';
export interface AccountView {
  id: string; name: string; provider: AgentProviderKind; profile_id: string; storage_path: string;
  native_source_id?: string | null;
  status: 'unknown' | 'signed_out' | 'signed_in' | 'login_pending' | 'error';
  email?: string; plan?: string; checked_at?: string; error?: string; login_command?: string; guidance?: string;
  login?: { url: string; user_code?: string; id?: string };
  limits?: {
    primary?: AccountLimit;
    secondary?: AccountLimit;
    reset_credits?: number;
    reset_credits_source?: 'official';
  };
  reset_outcome?: AccountResetOutcome;
  usage?: AccountUsage;
  /** Proxy for this account's own sign-in and usage requests; never carries credentials. */
  proxy_url?: string;
  capabilities: { login: boolean; refresh_token: boolean; quota: boolean; reset_quota: boolean; logout: boolean; usage?: boolean };
}
export function accountStatusLabel(status: AccountView['status']): string {
  return { unknown: 'Not checked', signed_out: 'Signed out', signed_in: 'Signed in', login_pending: 'Sign-in in progress', error: 'Account needs attention' }[status];
}
export function accountLimitPercent(limit?: AccountLimit): number | undefined {
  return limit && Number.isFinite(limit.used_percent) && limit.used_percent >= 0 && limit.used_percent <= 100 ? limit.used_percent : undefined;
}
/**
 * Return a label that makes the primary/secondary -> 5h/weekly mapping
 * explicit. Unknown durations intentionally remain the native slot name.
 */
export function accountWindowLabel(slot: 'primary' | 'secondary', limit?: AccountLimit): {
  slot: string;
  kind?: AccountWindowKind;
  minutes?: number;
} {
  return { slot, ...(limit?.window_kind ? { kind: limit.window_kind } : {}), ...(limit?.window_minutes !== undefined ? { minutes: limit.window_minutes } : {}) };
}
export function accountResetCreditState(account: Pick<AccountView, 'status' | 'capabilities' | 'limits'>):
  'unsupported' | 'signed_out' | 'not_reported' | 'available' | 'empty' {
  if (!account.capabilities.quota) return 'unsupported';
  if (account.status !== 'signed_in') return 'signed_out';
  const count = account.limits?.reset_credits;
  if (count === undefined || account.limits?.reset_credits_source !== 'official') return 'not_reported';
  return count > 0 ? 'available' : 'empty';
}
export function accountResetDate(value?: string | number): Date | undefined {
  if (value === undefined) return undefined;
  const date = new Date(typeof value === 'number' && value < 1e12 ? value * 1000 : value);
  return Number.isFinite(date.getTime()) ? date : undefined;
}
export function accountLoginUrl(account: Pick<AccountView, 'provider' | 'login'>): string | undefined {
  if (!account.login?.url) return undefined;
  try {
    const url = new URL(account.login.url), domains = account.provider === 'codex' ? ['openai.com', 'chatgpt.com'] : ['claude.ai', 'anthropic.com'];
    return url.protocol === 'https:' && !url.username && !url.password && domains.some(domain => url.hostname === domain || url.hostname.endsWith('.' + domain)) ? url.href : undefined;
  } catch { return undefined; }
}
export function canResetAccountQuota(account: AccountView): boolean {
  return account.status === 'signed_in' && account.capabilities.reset_quota && account.capabilities.quota && Number.isFinite(account.limits?.reset_credits) && Number(account.limits?.reset_credits) > 0;
}
export function quotaResetOutcomeNotice(outcome?: AccountResetOutcome): string {
  return outcome === 'reset' ? 'Official quota reset completed.' : outcome === 'alreadyRedeemed' ? 'This reset credit was already redeemed. No new quota reset was performed.' : outcome === 'nothingToReset' ? 'The official provider reports nothing to reset. No quota reset was performed.' : outcome === 'noCredit' ? 'No official reset credit is available. No quota reset was performed.' : 'The reset request completed. The official status below is the source of truth.';
}
export interface QuotaResetAttempt { idempotency_key: string; credit_id?: string; outcome: 'pending' | 'unknown' }
export const QUOTA_RESET_STORAGE_KEY = 'agentdock.pending-quota-resets.v1';
export const QUOTA_RESET_STORAGE_WARNING = 'Pending reset protection is unavailable in this browser. Do not repeat a quota reset; refresh official quota first. New reset requests are blocked.';
type ResetStorage = Pick<Storage, 'getItem' | 'setItem'>;
export function createQuotaResetStore(uuid: () => string = () => crypto.randomUUID(), storage?: ResetStorage | (() => ResetStorage | undefined), requirePersistence = false) {
  const attempts = new Map<string, QuotaResetAttempt>(), durable = new Set<string>();
  let hydrated = false, storageFailed = false;
  function resolveStorage() { try { return typeof storage === 'function' ? storage() : storage; } catch { return undefined; } }
  function hydrate() {
    if (hydrated) return;
    const source = resolveStorage();
    if (!source) { storageFailed = requirePersistence; hydrated = true; return; }
    try {
      const raw = source.getItem(QUOTA_RESET_STORAGE_KEY), values: unknown = raw ? JSON.parse(raw) : [];
      if (!Array.isArray(values) || values.length > 1000 || values.some(item => !item || typeof item.account_id !== 'string' || !/^[A-Za-z0-9_-]{1,200}$/.test(item.account_id) || typeof item.idempotency_key !== 'string' || !/^[A-Za-z0-9_-]{1,200}$/.test(item.idempotency_key))) throw Error('Invalid pending reset metadata');
      for (const item of values) { attempts.set(item.account_id, { idempotency_key: item.idempotency_key, outcome: 'unknown' }); durable.add(item.account_id); }
    } catch { storageFailed = true; }
    hydrated = true;
  }
  function persist(omit?: string) {
    const source = resolveStorage();
    if (!source) { storageFailed = requirePersistence; return !requirePersistence; }
    try {
      // Only opaque account/request identifiers survive a page reload. Never
      // store account details, login URLs, tokens, quotas or secret values.
      source.setItem(QUOTA_RESET_STORAGE_KEY, JSON.stringify([...attempts].filter(([id]) => id !== omit).map(([account_id, attempt]) => ({ account_id, idempotency_key: attempt.idempotency_key }))));
      storageFailed = false; return true;
    } catch { storageFailed = true; return false; }
  }
  return {
    get(id: string) { hydrate(); return attempts.get(id); },
    storageAvailable() { hydrate(); return !storageFailed; },
    begin(account: AccountView, confirmed: boolean, creditId?: string) {
      if (!confirmed) throw Error('Confirm that this reset may consume an official reset credit.');
      hydrate();
      const existing = attempts.get(account.id);
      if (existing) { if (requirePersistence && !durable.has(account.id) && !persist()) throw Error(QUOTA_RESET_STORAGE_WARNING); durable.add(account.id); return existing; }
      if (!canResetAccountQuota(account)) throw Error('The official client does not currently offer a quota reset credit.');
      // A denied/corrupt/read-only storage cannot silently generate a new key
      // after a reload and charge for a second, unknowingly repeated request.
      if (requirePersistence && (storageFailed || !persist())) throw Error(QUOTA_RESET_STORAGE_WARNING);
      const attempt: QuotaResetAttempt = { idempotency_key: uuid(), outcome: 'pending', ...(creditId ? { credit_id: creditId } : {}) };
      attempts.set(account.id, attempt);
      if (requirePersistence && !persist()) throw Error(QUOTA_RESET_STORAGE_WARNING);
      durable.add(account.id); return attempt;
    },
    unknown(id: string) { const attempt = attempts.get(id); if (attempt) attempt.outcome = 'unknown'; },
    complete(id: string) { hydrate(); if (requirePersistence && !persist(id)) { const attempt = attempts.get(id); if (attempt) attempt.outcome = 'unknown'; return false; } attempts.delete(id); durable.delete(id); return true; },
  };
}
export const quotaResetAttempts = createQuotaResetStore(() => crypto.randomUUID(), () => typeof window === 'undefined' ? undefined : window.sessionStorage, true);
