// Official account APIs only: no model requests, token exchange, or locally
// manufactured rate-limit resets. A user-triggered usage check may read the
// official client's OAuth credential from its native store in memory so it can
// call the provider's own usage endpoint; the credential never leaves here.
import { spawn } from 'node:child_process';
import { realpath, stat, readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';

const clean = (value, length = 240) => typeof value === 'string' ? value.replace(/[\u0000-\u001f\u007f]/g, ' ').slice(0, length) : undefined;
const numeric = value => typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined;
export const noCapabilities = () => ({ login: false, refresh_token: false, quota: false, reset_quota: false, logout: false });
export function isolatedEnvironment(parent, provider, directory, configEnv = directory) {
  const entries = Object.entries(parent).filter(([key]) => !/^(AGENTDOCK_|OPENAI_|ANTHROPIC_|CLAUDE_|CLAUDECODE$|CODEX_|AZURE_OPENAI_)/i.test(key));
  const environment = Object.fromEntries(entries);
  const key = provider === 'codex' ? 'CODEX_HOME' : 'CLAUDE_CONFIG_DIR';
  return configEnv ? { ...environment, [key]: configEnv } : environment;
}
export function safeLoginUrl(value) {
  try { const url = new URL(value); return url.protocol === 'https:' && !url.username && !url.password && ['openai.com', 'chatgpt.com'].some(host => url.hostname === host || url.hostname.endsWith('.' + host)) ? url.href : undefined; } catch { return undefined; }
}
export function normalizeLimits(response) {
  const limits = response?.rateLimitsByLimitId?.codex ?? response?.rateLimits;
  // Codex currently exposes opaque `primary`/`secondary` keys rather than
  // user-facing names. We only attach a friendly kind when the official
  // duration is an exact known window; unknown durations stay unmapped.
  const window = value => {
    if (!value || numeric(value.usedPercent) === undefined) return undefined;
    const minutes = numeric(value.windowDurationMins);
    const windowKind = minutes === 300 ? 'five_hour' : minutes === 10080 ? 'weekly' : undefined;
    return {
      used_percent: value.usedPercent,
      ...(minutes !== undefined ? { window_minutes: minutes } : {}),
      ...(windowKind ? { window_kind: windowKind, mapping_basis: 'duration_minutes' } : {}),
      ...(numeric(value.resetsAt) !== undefined ? { resets_at: value.resetsAt } : {}),
    };
  };
  const primary = window(limits?.primary), secondary = window(limits?.secondary);
  const resetCredits = response?.rateLimitResetCredits;
  const count = resetCredits?.availableCount;
  const available = Number.isSafeInteger(count) && count >= 0 ? count : undefined;
  // A count is only considered official when it came from the native
  // rateLimitResetCredits object. Keeping the provenance lets the UI explain
  // that this is an earned reset credit, never a locally-created reset.
  const resetSource = resetCredits && typeof resetCredits === 'object' && !Array.isArray(resetCredits) ? 'official' : undefined;
  return {
    ...(primary ? { primary } : {}),
    ...(secondary ? { secondary } : {}),
    ...(available !== undefined ? { reset_credits: available } : {}),
    ...(resetSource ? { reset_credits_source: resetSource } : {}),
  };
}
// Claude Code publishes subscription usage to its status line command as a
// documented JSON payload on stdin. The explicit usage action prefers the
// official OAuth usage endpoint and falls back to this local capture.
export function normalizeClaudeLimits(payload, mappingBasis = 'status_line_window') {
  const limits = payload?.rate_limits ?? payload;
  if (!limits || typeof limits !== 'object' || Array.isArray(limits)) return {};
  const window = value => {
    if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
    const explicitPercent = numeric(value.used_percentage);
    const utilization = numeric(value.utilization);
    // CPA/CLIProxyAPI's official usage response uses utilization as a
    // percentage (36), while some native status payloads call the same field
    // used_percentage. Accept a fractional utilization defensively too.
    const percent = explicitPercent !== undefined ? explicitPercent : utilization === undefined ? undefined : utilization <= 1 ? utilization * 100 : utilization;
    if (percent === undefined || percent > 100) return undefined;
    return {
      used_percent: percent,
      ...(resetTimestamp(value.resets_at) !== undefined ? { resets_at: resetTimestamp(value.resets_at) } : {}),
    };
  };
  const fiveHour = window(limits.five_hour), weekly = window(limits.seven_day), spend = window(limits.spend_limit);
  // Older AgentDock servers only know the status_line_window enum. OAuth
  // provenance is carried by usage.source, so omitting the new enum keeps a hot
  // old server able to deserialize a result while it still owns live sessions.
  if (mappingBasis !== 'oauth_usage') {
    if (fiveHour) { fiveHour.window_minutes = 300; fiveHour.window_kind = 'five_hour'; fiveHour.mapping_basis = mappingBasis; }
    if (weekly) { weekly.window_minutes = 10080; weekly.window_kind = 'weekly'; weekly.mapping_basis = mappingBasis; }
  } else {
    if (fiveHour) { fiveHour.window_minutes = 300; fiveHour.window_kind = 'five_hour'; }
    if (weekly) { weekly.window_minutes = 10080; weekly.window_kind = 'weekly'; }
  }
  return {
    ...(fiveHour ? { primary: fiveHour } : {}),
    ...(weekly ? { secondary: weekly } : {}),
    ...(spend ? { spend: spend } : {}),
  };
}
export function claudeStatusPath(directory, configEnv) {
  return join(configEnv ?? directory, 'agentdock', 'statusline.json');
}

const CLAUDE_OAUTH_USAGE_URL = 'https://api.anthropic.com/api/oauth/usage';
const MAX_CLAUDE_CREDENTIAL_BYTES = 512 * 1024;

function claudeOAuthCredential(payload) {
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return undefined;
  const candidates = [payload.claudeAiOauth, payload.claude_ai_oauth, payload.oauth, payload];
  for (const candidate of candidates) {
    if (!candidate || typeof candidate !== 'object' || Array.isArray(candidate)) continue;
    const accessToken = candidate.accessToken ?? candidate.access_token;
    if (typeof accessToken === 'string' && accessToken.trim()) {
      return {
        accessToken: accessToken.trim(),
        ...(typeof candidate.refreshToken === 'string' && candidate.refreshToken.trim() ? { refreshToken: candidate.refreshToken.trim() } : {}),
        ...(numeric(candidate.expiresAt) !== undefined ? { expiresAt: candidate.expiresAt } : {}),
      };
    }
  }
  return undefined;
}

function resetTimestamp(value) {
  if (numeric(value) !== undefined) return value;
  if (typeof value === 'string' && Number.isFinite(Date.parse(value))) return Date.parse(value) / 1000;
  return undefined;
}

function quietCommand(program, args, cwd, timeoutMs = 4000) {
  return new Promise(resolveCommand => {
    let settled = false, output = '';
    const finish = value => { if (settled) return; settled = true; clearTimeout(timer); resolveCommand(value); };
    const child = spawn(program, args, { cwd, stdio: ['ignore', 'pipe', 'ignore'] });
    const timer = setTimeout(() => { child.kill('SIGKILL'); finish(undefined); }, timeoutMs);
    child.stdout.on('data', chunk => {
      if (Buffer.byteLength(output) + chunk.length > MAX_CLAUDE_CREDENTIAL_BYTES) { child.kill('SIGKILL'); finish(undefined); return; }
      output += chunk.toString('utf8');
    });
    child.once('error', () => finish(undefined));
    child.once('close', code => finish(code === 0 ? output : undefined));
  });
}

async function readClaudeOAuthCredential(job) {
  // macOS Claude Code stores the claudeAiOauth object in the official Keychain.
  // The value is consumed in memory only; it is never returned by the bridge.
  // Only the default native home may use the shared Keychain item. An isolated
  // AgentDock account must not accidentally authenticate as the host account.
  const defaultHome = join(homedir(), '.claude');
  if (process.platform === 'darwin' && !job.config_env && resolve(job.config_dir) === resolve(defaultHome)) {
    const raw = await quietCommand('security', ['find-generic-password', '-s', 'Claude Code-credentials', '-w'], job.config_dir);
    if (raw) { try { const credential = claudeOAuthCredential(JSON.parse(raw)); if (credential) return credential; } catch { /* Try the platform file fallback. */ } }
  }
  // Linux and older installations can keep the same object in a local file.
  for (const path of [join(job.config_dir, '.credentials.json'), join(job.config_dir, 'credentials.json')]) {
    try {
      const raw = await readFile(path, { encoding: 'utf8', flag: 'r' });
      if (Buffer.byteLength(raw) > MAX_CLAUDE_CREDENTIAL_BYTES) continue;
      const credential = claudeOAuthCredential(JSON.parse(raw));
      if (credential) return credential;
    } catch { /* The next official storage location may still be usable. */ }
  }
  return undefined;
}

/**
 * The usage endpoint answers only Claude Code: a request without its user
 * agent is refused with 429 every time, which read as a rate limit that never
 * cleared. The installed client's own version is sent; if it cannot be read,
 * a recent one stands in.
 */
const FALLBACK_CLAUDE_VERSION = '2.1.0';
const claudeVersions = new Map();
async function claudeUserAgent(job) {
  const program = job.program || 'claude';
  if (!claudeVersions.has(program)) {
    const output = await quietCommand(program, ['--version'], job.config_dir);
    claudeVersions.set(program, /^\s*(\d+\.\d+\.\d+)/.exec(output ?? '')?.[1] ?? FALLBACK_CLAUDE_VERSION);
  }
  return `claude-code/${claudeVersions.get(program)}`;
}

async function fetchClaudeOAuthUsage(accessToken, userAgent) {
  const controller = new AbortController(), timer = setTimeout(() => controller.abort(), 12000);
  try {
    const response = await fetch(CLAUDE_OAUTH_USAGE_URL, {
      method: 'GET',
      redirect: 'error',
      signal: controller.signal,
      headers: {
        Accept: 'application/json',
        'Content-Type': 'application/json',
        Authorization: `Bearer ${accessToken}`,
        'anthropic-beta': 'oauth-2025-04-20',
        'User-Agent': userAgent,
      },
    });
    if (!response.ok) throw new UsageQueryError(response.status, response.headers?.get?.('retry-after'));
    const raw = await response.text();
    if (Buffer.byteLength(raw) > MAX_CLAUDE_CREDENTIAL_BYTES) throw Error('response too large');
    return JSON.parse(raw);
  } finally { clearTimeout(timer); }
}

/**
 * A failed official usage query. The status class is kept so the account page
 * can say what actually happened; the response body is never carried, because
 * it is provider diagnostics rather than something the user can act on.
 */
export class UsageQueryError extends Error {
  constructor(status, retryAfter) {
    super(`Official usage query returned HTTP ${status}.`);
    this.status = status;
    const seconds = retryAfterSeconds(retryAfter);
    if (seconds !== undefined) this.retryAfter = seconds;
  }
}
/** Retry-After is either a delta in seconds or an HTTP date; accept both. */
export function retryAfterSeconds(header, now = Date.now()) {
  if (typeof header !== 'string' || !header.trim()) return undefined;
  const raw = header.trim();
  const delta = /^\d+$/.test(raw) ? Number(raw) : Number.isFinite(Date.parse(raw)) ? Math.round((Date.parse(raw) - now) / 1000) : undefined;
  if (delta === undefined || !Number.isFinite(delta)) return undefined;
  // Clamp to a day: a negative or absurd value would only mislead the user.
  return delta <= 0 ? 0 : Math.min(delta, 86400);
}
/**
 * Classify a usage-query failure. The distinction matters: a rate limit clears
 * by itself, an expired credential needs a native sign-in, and only a genuine
 * transport error is worth pointing at the network.
 */
export function usageFailure(cause) {
  const status = cause instanceof UsageQueryError ? cause.status : undefined;
  const retry = cause instanceof UsageQueryError ? cause.retryAfter : undefined;
  const availability = status === 429 ? 'rate_limited' : status === 401 || status === 403 ? 'unauthorized' : 'query_failed';
  return { availability, ...(retry !== undefined ? { retry_after_seconds: retry } : {}), ...(status !== undefined ? { status } : {}) };
}

export class RpcError extends Error {
  constructor(code) { super(code === -32601 ? 'Installed client does not support this official account operation.' : 'Official account operation failed.'); this.code = code; }
}
class RpcClient {
  constructor(job) {
    this.child = spawn(job.program || 'codex', ['app-server'], { cwd: job.config_dir, env: isolatedEnvironment(process.env, 'codex', job.config_dir, job.config_env), stdio: ['pipe', 'pipe', 'pipe'] });
    this.pending = new Map(); this.notifications = []; this.waiters = new Set(); this.sequence = 0; this.closed = false;
    this.child.stderr.on('data', () => {}); // Never reflect native auth diagnostics.
    this.child.stdin.on('error', () => this.fail());
    this.child.once('error', () => this.fail()); this.child.once('exit', () => this.fail());
    let buffer = '', total = 0;
    this.child.stdout.setEncoding('utf8');
    this.child.stdout.on('data', chunk => {
      total += Buffer.byteLength(chunk); buffer += chunk;
      if (total > 4 * 1024 * 1024 || buffer.length > 1024 * 1024) { this.fail(); this.child.kill(); return; }
      let newline;
      while ((newline = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
        let message; try { message = JSON.parse(line); } catch { continue; }
        if (message.id !== undefined && this.pending.has(message.id)) {
          const pending = this.pending.get(message.id); this.pending.delete(message.id);
          message.error ? pending.reject(new RpcError(message.error.code)) : pending.resolve(message.result);
        } else if (message.method === 'account/login/completed') {
          this.notifications.push(message.params); this.notifications = this.notifications.slice(-16);
          for (const wake of this.waiters) wake();
        } else if (message.id !== undefined && message.method) {
          this.child.stdin.write(JSON.stringify({ id: message.id, error: { code: -32601, message: 'External token hosting is not supported' } }) + '\n');
        }
      }
    });
  }
  fail() { for (const value of this.pending.values()) value.reject(new Error('Native account process closed.')); this.pending.clear(); this.closed = true; for (const wake of this.waiters) wake(); }
  request(method, params = {}, signal) {
    if (signal?.aborted || this.closed) return Promise.reject(new Error('Account operation cancelled.'));
    return new Promise((resolveRequest, rejectRequest) => {
      const id = ++this.sequence;
      const finish = (callback, value) => { clearTimeout(timer); signal?.removeEventListener('abort', abort); this.pending.delete(id); callback(value); };
      const abort = () => finish(rejectRequest, new Error('Account operation cancelled.'));
      const timer = setTimeout(() => finish(rejectRequest, new Error('Official account request timed out.')), 20000);
      this.pending.set(id, { resolve: value => finish(resolveRequest, value), reject: error => finish(rejectRequest, error) });
      signal?.addEventListener('abort', abort, { once: true });
      this.child.stdin.write(JSON.stringify({ id, method, params }) + '\n', error => { if (error) finish(rejectRequest, new Error('Native account input closed.')); });
    });
  }
  notify(method) { this.child.stdin.write(JSON.stringify({ method, params: {} }) + '\n'); }
  waitLogin(id, signal) {
    return new Promise((resolveWait, rejectWait) => {
      const finish = (callback, value) => { clearTimeout(timer); this.waiters.delete(check); signal?.removeEventListener('abort', check); callback(value); };
      const check = () => {
        const found = this.notifications.find(value => value?.loginId === id);
        if (found) finish(resolveWait, found);
        else if (signal?.aborted) finish(resolveWait, { cancelled: true });
        else if (this.closed) finish(rejectWait, new Error('Native account process closed.'));
      };
      const timer = setTimeout(() => finish(resolveWait, { cancelled: true, expired: true }), 30 * 60 * 1000);
      this.waiters.add(check); signal?.addEventListener('abort', check, { once: true }); check();
    });
  }
  async close() {
    this.child.stdin.end(); this.child.kill('SIGTERM');
    await new Promise(resolveClose => {
      if (this.child.exitCode !== null || this.child.signalCode) { resolveClose(); return; }
      const timer = setTimeout(() => { this.child.kill('SIGKILL'); resolveClose(); }, 1000);
      this.child.once('exit', () => { clearTimeout(timer); resolveClose(); });
    });
  }
}

async function readCodex(client, refresh, signal) {
  const response = await client.request('account/read', { refreshToken: !!refresh }, signal);
  const account = response?.account;
  if (!response || !Object.hasOwn(response, 'account') || (account !== null && (!account || !['chatgpt', 'apiKey', 'amazonBedrock'].includes(account.type)))) throw new RpcError(-32601);
  const view = { status: account ? 'signed_in' : 'signed_out', checked_at: new Date().toISOString(), capabilities: { login: true, refresh_token: true, quota: false, reset_quota: false, logout: true } };
  if (account) { view.email = clean(account.email); view.plan = clean(account.planType, 80); }
  if (account?.type === 'chatgpt') {
    try {
      const limits = await client.request('account/rateLimits/read', {}, signal);
      view.limits = normalizeLimits(limits); view.capabilities.quota = true;
      view.capabilities.reset_quota = (view.limits.reset_credits ?? 0) > 0;
    } catch { view.error = 'Official quota information is unavailable for this client or account.'; }
  }
  return view;
}

async function claudeStatus(job) {
  if (job.action !== 'read' || job.refresh_token) throw new RpcError(-32601);
  const child = spawn(job.program || 'claude', ['auth', 'status', '--json'], { cwd: job.config_dir, env: isolatedEnvironment(process.env, 'claude_code', job.config_dir, job.config_env), stdio: ['ignore', 'pipe', 'ignore'] });
  const result = await new Promise((resolveStatus, rejectStatus) => {
    let data = ''; const timer = setTimeout(() => { child.kill('SIGKILL'); rejectStatus(new Error('Status timed out.')); }, 15000);
    child.stdout.setEncoding('utf8'); child.stdout.on('data', chunk => { data += chunk; if (data.length > 65536) { child.kill('SIGKILL'); clearTimeout(timer); rejectStatus(new Error('Status response exceeds limit.')); } });
    child.once('error', () => { clearTimeout(timer); rejectStatus(new Error('Native Claude client is unavailable.')); });
    child.once('exit', () => { clearTimeout(timer); try { resolveStatus(JSON.parse(data)); } catch { rejectStatus(new RpcError(-32601)); } });
  });
  if (typeof result.loggedIn !== 'boolean') throw new RpcError(-32601);
  const capabilities = { ...noCapabilities(), usage: true };
  return { status: result.loggedIn ? 'signed_in' : 'signed_out', email: clean(result.email), plan: clean(result.subscriptionType, 80), checked_at: new Date().toISOString(), capabilities };
}

/// Explicit, user-triggered usage check for Claude Code. Prefer the same
/// official OAuth usage endpoint used by CPA/CLIProxyAPI, then fall back to a
/// local status-line capture when the native credential store is unavailable.
async function claudeUsage(job) {
  const view = await claudeStatus({ ...job, action: 'read' });
  const credential = await readClaudeOAuthCredential(job);
  let failure;
  if (credential?.accessToken && view.status === 'signed_in') {
    try {
      const limits = normalizeClaudeLimits(await fetchClaudeOAuthUsage(credential.accessToken, await claudeUserAgent(job)), 'oauth_usage');
      return {
        ...view,
        limits,
      usage: {
          availability: Object.keys(limits).length === 0 ? 'not_reported' : 'available',
          source: 'claude_oauth_usage',
          ...(credential.expiresAt !== undefined ? { credential_expires_at: credential.expiresAt } : {}),
        },
      };
    } catch (cause) {
      // The body is never exposed, but the reason is: a swallowed failure left
      // the page blank with nothing the user could act on. A local capture may
      // still be usable, so continue to the native status-line fallback below.
      failure = usageFailure(cause);
    }
  }
  const path = claudeStatusPath(job.config_dir, job.config_env);
  let payload;
  try {
    const raw = await readFile(path, 'utf8');
    if (raw.length > 262144) throw new Error('too large');
    payload = JSON.parse(raw);
  } catch {
    const { status: _status, ...reported } = failure ?? {};
    return {
      ...view,
      limits: {},
      usage: {
        // A real query failure names itself. Only when no query ran do we fall
        // back to describing the missing credential or capture.
        availability: credential ? 'query_failed' : job.config_env ? 'capture_missing' : 'credential_unavailable',
        capture_path: path,
        ...reported,
      },
    };
  }
  const limits = normalizeClaudeLimits(payload);
  const capturedAt = numeric(payload?.captured_at);
  const resetTimestamp = value => numeric(value) !== undefined ? value : typeof value === 'string' && Number.isFinite(Date.parse(value)) ? Date.parse(value) / 1000 : undefined;
  const resetsAt = [limits.primary?.resets_at, limits.secondary?.resets_at, limits.spend?.resets_at].map(resetTimestamp).filter(value => value !== undefined);
  // The capture is a snapshot, so stale windows are reported as stale rather
  // than shown as current. A window whose resets_at already passed is dropped.
  const now = Date.now() / 1000;
  const fresh = resetsAt.length === 0 || resetsAt.some(value => value > now);
  return {
    ...view,
    limits,
    usage: {
      availability: Object.keys(limits).length === 0 ? 'not_reported' : fresh ? 'available' : 'stale',
      source: 'status_line_capture',
      capture_path: path,
      ...(capturedAt !== undefined ? { captured_at: capturedAt } : {}),
    },
  };
}

export async function executeAccount(job, { emit = () => {}, clientFactory = value => new RpcClient(value), signal, authorizeReset = async () => {} } = {}) {
  if (!job || !['codex', 'claude_code'].includes(job.provider) || !['read', 'usage', 'login', 'logout', 'reset_quota'].includes(job.action)) throw new Error('Invalid account operation.');
  if (job.provider === 'claude_code') { const view = await (job.action === 'usage' ? claudeUsage(job) : claudeStatus(job)); emit({ type: 'view', view }); return view; }
  const client = clientFactory(job);
  try {
    await client.request('initialize', { clientInfo: { name: 'agentdock_accounts', title: 'AgentDock account management', version: '0.1.0' } }, signal);
    client.notify('initialized');
    if (job.action === 'login') {
      if (!['device', 'browser'].includes(job.mode)) throw new Error('Choose a supported login mode.');
      const login = await client.request('account/login/start', { type: job.mode === 'device' ? 'chatgptDeviceCode' : 'chatgpt' }, signal);
      const url = safeLoginUrl(login?.verificationUrl ?? login?.authUrl);
      if (!url || typeof login.loginId !== 'string' || !login.loginId) throw new Error('Native login did not return a safe official authorization URL.');
      emit({ type: 'login', login: { url, id: clean(login.loginId, 128), ...(login.userCode ? { user_code: clean(login.userCode, 128) } : {}) } });
      const result = await client.waitLogin(login.loginId, signal);
      if (result.cancelled) {
        try { await client.request('account/login/cancel', { loginId: login.loginId }); } catch { /* Closing the native process also closes this attempt. */ }
      } else if (!result.success) throw new Error('Official login did not complete successfully.');
    } else if (job.action === 'logout') {
      if (!job.confirmed) throw new Error('Logout requires explicit confirmation.');
      await client.request('account/logout', {}, signal);
    } else if (job.action === 'reset_quota') {
      if (!job.confirmed || typeof job.idempotency_key !== 'string' || !job.idempotency_key.trim() || job.idempotency_key.length > 200) throw new Error('Quota reset requires confirmation and a stable idempotency key.');
      const available = await client.request('account/rateLimits/read', {}, signal);
      if (!job.retry_authorized && (normalizeLimits(available).reset_credits ?? 0) <= 0) throw new Error('No official earned rate-limit reset credits are available.');
      if (!job.retry_authorized && job.credit_id && !available.rateLimitResetCredits?.credits?.some(credit => credit.id === job.credit_id && credit.status === 'available')) throw new Error('The selected official reset credit is not available.');
      emit({ type: 'reset_authorized', idempotency_key: job.idempotency_key });
      await authorizeReset(job.idempotency_key);
      const result = await client.request('account/rateLimitResetCredit/consume', { idempotencyKey: job.idempotency_key, ...(job.credit_id ? { creditId: job.credit_id } : {}) }, signal);
      if (!['reset', 'alreadyRedeemed', 'nothingToReset', 'noCredit'].includes(result?.outcome)) throw new Error('Unrecognized official reset result.');
      emit({ type: 'reset', outcome: result.outcome });
    }
    const view = await readCodex(client, job.action === 'read' && job.refresh_token, job.action === 'login' && signal?.aborted ? undefined : signal);
    emit({ type: 'view', view }); return view;
  } finally { await client.close(); }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  let started = false, done = false, job, pendingReset;
  const controller = new AbortController();
  const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
  const emit = message => process.stdout.write(JSON.stringify(message) + '\n');
  const cancel = () => controller.abort(); process.once('SIGTERM', cancel); process.once('SIGINT', cancel);
  input.on('line', line => {
    if (line.length > 32768) { controller.abort(); return; }
    if (started) { try { const message = JSON.parse(line); if (message.type === 'cancel') controller.abort(); else if (message.type === 'reset_authorized_ack' && message.idempotency_key === pendingReset?.key) pendingReset.resolve(); } catch { /* Ignore invalid controls. */ } return; }
    started = true;
    void (async () => {
      try {
        job = JSON.parse(line);
        const directory = await realpath(job.config_dir);
        if (directory !== resolve(job.config_dir) || !(await stat(directory)).isDirectory()) throw new Error('Account directory is unavailable.');
        const authorizeReset = key => new Promise((resolveAuthorization, rejectAuthorization) => {
          const finish = callback => { clearTimeout(timer); controller.signal.removeEventListener('abort', abort); pendingReset = undefined; callback(); };
          const abort = () => finish(() => rejectAuthorization(new Error('Reset authorization cancelled.')));
          const timer = setTimeout(() => finish(() => rejectAuthorization(new Error('Reset authorization was not persisted.'))), 10000);
          pendingReset = { key, resolve: () => finish(resolveAuthorization) };
          controller.signal.addEventListener('abort', abort, { once: true });
          if (controller.signal.aborted) abort();
        });
        await executeAccount(job, { emit, signal: controller.signal, authorizeReset });
      } catch (error) {
        emit({ type: 'view', view: { status: error instanceof RpcError && error.code === -32601 ? 'unknown' : 'error', error: error instanceof RpcError ? error.message : 'Official account operation did not complete. Check native client availability and try again.', checked_at: new Date().toISOString(), capabilities: noCapabilities() } });
      } finally { done = true; input.close(); process.stdin.destroy(); process.removeListener('SIGTERM', cancel); process.removeListener('SIGINT', cancel); }
    })();
  });
  input.on('close', () => { if (!done && job?.action === 'login') controller.abort(); });
}
