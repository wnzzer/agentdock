import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { nativeCommand, shimTarget, spawnNative, withoutAgentDockSecrets } from './native-spawn.mjs';

test('only AgentDock credentials are withheld from a native client', () => {
  const env = { PATH: '/bin', AGENTDOCK_SECRET_WORK: 's', AGENTDOCK_TOKEN: 't', AGENTDOCK_HOME: '/h', AGENTDOCK_TOKEN_FILE: 'f', OPENAI_API_KEY: 'k' };
  assert.deepEqual(withoutAgentDockSecrets(env), { PATH: '/bin', AGENTDOCK_HOME: '/h', AGENTDOCK_TOKEN_FILE: 'f', OPENAI_API_KEY: 'k' });
});

test('outside Windows a client is started exactly as named', () => {
  assert.deepEqual(nativeCommand('claude', ['--name', 'a & b'], { platform: 'linux' }), { command: 'claude', args: ['--name', 'a & b'] });
});

// `.cmd` shims, and the `\` separators they are written with, exist only on Windows.
test('an npm shim runs its script with node and never passes through cmd.exe', { skip: process.platform !== 'win32' }, async t => {
  const directory = await mkdtemp(join(tmpdir(), 'agentdock-shim-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await mkdir(join(directory, 'node_modules', '@scope', 'tool'), { recursive: true });
  const script = join(directory, 'node_modules', '@scope', 'tool', 'cli.js');
  await writeFile(script, '');
  const npm = join(directory, 'tool.cmd');
  await writeFile(npm, '@ECHO off\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & "%_prog%"  "%dp0%\\node_modules\\@scope\\tool\\cli.js" %*\r\n');
  const pnpm = join(directory, 'pnpm-tool.cmd');
  await writeFile(pnpm, '@IF EXIST "%~dp0\\node.exe" (\r\n  "%~dp0\\node.exe"  "%~dp0\\node_modules\\@scope\\tool\\cli.js" %*\r\n)\r\n');
  assert.equal(shimTarget(npm), script);
  assert.equal(shimTarget(pnpm), script);

  const args = ['--name', 'a & calc'];
  const launched = nativeCommand(npm, args, { platform: 'win32', node: 'node-under-test' });
  assert.deepEqual(launched, { command: 'node-under-test', args: [script, ...args] });

  // A batch file that is not a shim is refused, surfacing as the child's error.
  const plain = join(directory, 'plain.bat');
  await writeFile(plain, '@echo %*\r\n');
  assert.throws(() => nativeCommand(plain, args, { platform: 'win32' }), /not an npm shim/);
});

test('a client that cannot be launched fails through the error event callers already handle', async () => {
  const child = spawnNative(join(tmpdir(), 'agentdock-missing-client-does-not-exist'), []);
  const error = await new Promise(resolve => child.once('error', resolve));
  assert.ok(error instanceof Error);
});
