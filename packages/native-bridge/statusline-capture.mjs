// Status line capture for Claude Code sessions AgentDock launches.
//
// Claude Code hands its status line command a documented JSON payload on stdin
// that includes `rate_limits` for subscription accounts. AgentDock stores that
// payload next to the session's configuration so the account page can show
// official usage when the user explicitly asks for it.
//
// This script reads stdin and writes one local file. It never reads
// credentials and never makes a network request. When the account already had
// its own status line command, that command is run with the same payload and
// its output is passed through unchanged, so the visible status line does not
// change just because a session was started from AgentDock.
import { spawn } from 'node:child_process';
import { mkdirSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const MAX_PAYLOAD = 262144;

function captureDirectory() {
  const configured = process.env.CLAUDE_CONFIG_DIR;
  return join(configured && configured.trim() ? configured : join(homedir(), '.claude'), 'agentdock');
}

function store(raw) {
  if (Buffer.byteLength(raw) > MAX_PAYLOAD) return;
  let payload;
  try { payload = JSON.parse(raw); } catch { return; }
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) return;
  const directory = captureDirectory();
  mkdirSync(directory, { recursive: true });
  // Written to a temporary name first so a reader never sees a partial file.
  const temporary = join(directory, `statusline.${process.pid}.tmp`);
  writeFileSync(temporary, JSON.stringify({ ...payload, captured_at: Math.floor(Date.now() / 1000) }), { mode: 0o600 });
  renameSync(temporary, join(directory, 'statusline.json'));
}

function delegate(raw) {
  const command = process.env.AGENTDOCK_STATUSLINE_DELEGATE;
  if (!command || !command.trim()) return Promise.resolve();
  return new Promise(resolve => {
    const child = spawn(command, { shell: true, stdio: ['pipe', 'inherit', 'ignore'] });
    const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
    const finish = () => { clearTimeout(timer); resolve(); };
    child.once('error', finish);
    child.once('exit', finish);
    child.stdin.on('error', () => {});
    child.stdin.end(raw);
  });
}

let raw = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => {
  raw += chunk;
  if (raw.length > MAX_PAYLOAD) { raw = raw.slice(0, MAX_PAYLOAD); process.stdin.destroy(); }
});
process.stdin.on('end', async () => {
  // A capture failure must never break the session's status line.
  try { store(raw); } catch { /* Usage simply stays uncaptured. */ }
  try { await delegate(raw); } catch { /* The delegate owns its own output. */ }
});
