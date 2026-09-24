// Starting a native client without a shell, on every platform.
//
// On Windows npm installs `claude` and `codex` as `.cmd` shims. Node refuses to
// spawn those with `shell: false` (EINVAL), and with `shell: true` cmd.exe would
// reparse the whole command line, so a session title such as `a & b` would run
// `b`. Instead the shim is read for the script it starts and that script runs
// under this same Node. A batch file that is not an npm shim is refused. This
// mirrors `agentdock_runtime::process::launcher` on the server.
import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { delimiter, dirname, extname, join, resolve } from 'node:path';

// AgentDock's own credentials never reach a native client. The same rule as
// `bridge::is_agentdock_secret` on the server, which already strips them from
// every bridge; repeated here so a bridge run by hand is held to it too.
export const isAgentDockSecret = key => key.startsWith('AGENTDOCK_SECRET_') || key === 'AGENTDOCK_TOKEN';
export const withoutAgentDockSecrets = env => Object.fromEntries(Object.entries(env).filter(([key]) => !isAgentDockSecret(key)));

const isFile = path => { try { return statSync(path).isFile(); } catch { return false; } };

export function findProgram(program, env = process.env, platform = process.platform) {
  if (/[\\/]/.test(program)) return isFile(program) ? program : undefined;
  const extensions = platform === 'win32'
    ? (extname(program) ? [''] : (env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';').filter(Boolean).map(e => e.toLowerCase()))
    : [''];
  for (const directory of (env.PATH || env.Path || '').split(delimiter)) {
    if (!directory) continue;
    for (const extension of extensions) {
      const candidate = join(directory, program + extension);
      if (isFile(candidate)) return candidate;
    }
  }
  return undefined;
}

// The script an npm or pnpm `.cmd` shim starts: the last quoted `%dp0%\…` or
// `%~dp0\…` path that is not Node itself.
export function shimTarget(shim) {
  let text;
  try { text = readFileSync(shim, 'utf8'); } catch { return undefined; }
  if (text.length > 16 * 1024) return undefined;
  let found;
  const quoted = text.split('"').filter((_, index) => index % 2 === 1);
  for (const value of quoted) {
    const match = /^%~?dp0%?[\\/]*(.*)$/i.exec(value);
    if (!match || !match[1] || match[1].toLowerCase() === 'node.exe') continue;
    const candidate = resolve(dirname(shim), match[1]);
    if (isFile(candidate)) found = candidate;
  }
  return found;
}

const SCRIPT = new Set(['.js', '.mjs', '.cjs']);

/**
 * @param {string} program
 * @param {string[]} args
 * @param {{ platform?: NodeJS.Platform, env?: NodeJS.ProcessEnv, node?: string }} [options]
 * @returns {{ command: string, args: string[] }}
 */
export function nativeCommand(program, args, { platform = process.platform, env = process.env, node = process.execPath } = {}) {
  if (platform !== 'win32') return { command: program, args };
  const path = findProgram(program, env, platform) || program;
  const extension = extname(path).toLowerCase();
  if (SCRIPT.has(extension)) return { command: node, args: [path, ...args] };
  if (extension === '.cmd' || extension === '.bat') {
    const target = shimTarget(path);
    if (!target) throw new Error(`${path} is a batch file that is not an npm shim; point the client override at its .exe or .js instead.`);
    if (!SCRIPT.has(extname(target).toLowerCase())) return { command: target, args };
    const local = join(dirname(path), 'node.exe');
    return { command: existsSync(local) ? local : node, args: [target, ...args] };
  }
  return { command: path, args };
}

// A client that cannot be launched fails the way a missing one does: through the
// child's 'error' event, which every caller already turns into its own message.
/** @param {unknown} error @returns {import('node:child_process').ChildProcess} */
function unlaunchable(error) {
  const stdout = new PassThrough(), stderr = new PassThrough();
  const child = Object.assign(new EventEmitter(), { stdin: new PassThrough(), stdout, stderr, pid: undefined, exitCode: null, signalCode: null, kill: () => false });
  process.nextTick(() => { child.emit('error', error); stdout.end(); stderr.end(); });
  return /** @type {any} */ (child);
}

/**
 * @param {string} program
 * @param {string[]} args
 * @param {import('node:child_process').SpawnOptions} [options]
 * @returns {import('node:child_process').ChildProcess}
 */
export function spawnNative(program, args, options = {}) {
  let launch;
  try { launch = nativeCommand(program, args); } catch (error) { return unlaunchable(error); }
  try { return spawn(launch.command, launch.args, { windowsHide: true, ...options, shell: false }); } catch (error) { return unlaunchable(error); }
}
