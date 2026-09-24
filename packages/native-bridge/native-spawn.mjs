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
function unlaunchable(error) {
  const child = new EventEmitter();
  Object.assign(child, { stdin: new PassThrough(), stdout: new PassThrough(), stderr: new PassThrough(), pid: undefined, exitCode: null, signalCode: null, kill: () => false });
  process.nextTick(() => { child.emit('error', error); child.stdout.end(); child.stderr.end(); });
  return child;
}

export function spawnNative(program, args, options = {}) {
  let launch;
  try { launch = nativeCommand(program, args); } catch (error) { return unlaunchable(error); }
  try { return spawn(launch.command, launch.args, { windowsHide: true, ...options, shell: false }); } catch (error) { return unlaunchable(error); }
}
