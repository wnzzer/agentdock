// Official account APIs only: no model requests, token exchange, or locally
// manufactured rate-limit resets. A user-triggered usage check may read the
// official client's OAuth credential from its native store in memory so it can
// call the provider's own usage endpoint; the credential never leaves here.
//
// This is the entry point and dispatcher; each client's operations live in its
// account adapter, named in clients.mjs.
import { CLIENTS, clientFor } from './clients.mjs';
import { noCapabilities, RpcError } from './account-common.mjs';
import { realpath, stat } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';

export function isolatedEnvironment(parent, provider, directory, configEnv = directory) {
  const entries = Object.entries(parent).filter(([key]) => !/^AGENTDOCK_/i.test(key) && !Object.values(CLIENTS).some(client => client.envStrip.test(key)));
  const environment = Object.fromEntries(entries);
  const key = clientFor(provider)?.configEnv;
  return configEnv && key ? { ...environment, [key]: configEnv } : environment;
}

/**
 * @param {any} job
 * @param {{ emit?: (event: object) => void, clientFactory?: (job: any) => import('./account-codex.mjs').AccountClient, signal?: AbortSignal, authorizeReset?: (key: string) => Promise<void> }} [options]
 */
export async function executeAccount(job, options = {}) {
  const client = clientFor(job?.provider);
  if (!job || !client || !['read', 'usage', 'login', 'logout', 'reset_quota'].includes(job.action)) throw new Error('Invalid account operation.');
  return client.account(job, { ...options, env: isolatedEnvironment(process.env, job.provider, job.config_dir, job.config_env) });
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
