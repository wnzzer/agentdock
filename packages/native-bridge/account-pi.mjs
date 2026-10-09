// Pi signs in inside Pi itself, with /login, and keeps what it stored in
// auth.json under its home. Reading whether that file holds any credential is
// all an account check does: the credentials themselves are never read out.
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { noCapabilities } from './account-common.mjs';

/** The providers Pi holds a credential for, by name only. */
async function signedInProviders(home) {
  try {
    const stored = JSON.parse(await readFile(join(home, 'auth.json'), 'utf8'));
    return stored && typeof stored === 'object' && !Array.isArray(stored) ? Object.keys(stored).filter(name => /^[\w.-]{1,64}$/.test(name)) : [];
  } catch { return []; }
}

/** @param {any} job @param {{ emit?: (event: object) => void, env?: Record<string, string|undefined> }} options */
export async function piAccount(job, { emit = () => {}, env }) {
  const home = env?.PI_CODING_AGENT_DIR ?? job.config_dir;
  const providers = await signedInProviders(home);
  const view = { status: providers.length ? 'signed_in' : 'signed_out', ...(providers.length ? { plan: providers.slice(0, 6).join(', ') } : {}), checked_at: new Date().toISOString(), capabilities: noCapabilities() };
  emit({ type: 'view', view }); return view;
}
