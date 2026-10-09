// Claude Code accounts: `claude auth status` for who is signed in, and the
// official OAuth usage endpoint (falling back to a status-line capture) for
// subscription usage. Sign-in, logout and quota resets stay in the native CLI.
import { spawnNative as spawn } from './native-spawn.mjs';
import { readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { homedir } from 'node:os';
import { clean, noCapabilities, numeric, RpcError } from './account-common.mjs';

// Claude Code publishes subscription usage to its status line command as a
// documented JSON payload on stdin. The explicit usage action prefers the
// official OAuth usage endpoint and falls back to this local capture.
export function normalizeClaudeLimits(payload, mappingBasis = 'status_line_window') {
  const limits = payload?.rate_limits ?? payload;
  if (!limits || typeof limits !== 'object' || Array.isArray(limits)) return {};
  /**
   * @param {any} value
   * @returns {{ used_percent: number, resets_at?: any, window_minutes?: number, window_kind?: string, mapping_basis?: string } | undefined}
   */
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

async function claudeStatus(job, env) {
  if (job.action !== 'read' || job.refresh_token) throw new RpcError(-32601);
  const child = spawn(job.program || 'claude', ['auth', 'status', '--json'], { cwd: job.config_dir, env, stdio: ['ignore', 'pipe', 'ignore'] });
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
async function claudeUsage(job, env) {
  const view = await claudeStatus({ ...job, action: 'read' }, env);
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

/** @param {any} job @param {{ emit?: (event: object) => void, env: NodeJS.ProcessEnv }} options */
export async function claudeAccount(job, { emit = () => {}, env }) {
  const view = await (job.action === 'usage' ? claudeUsage(job, env) : claudeStatus(job, env)); emit({ type: 'view', view }); return view;
}
