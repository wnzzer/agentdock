#!/usr/bin/env node
// Entry point for `@wnzzer/agentdock`. The real program is a Rust binary that
// ships in a per-platform package; npm installs only the one matching this
// machine, via the `os`/`cpu` fields on those packages. This file finds it and
// hands over.
//
// CommonJS on purpose: `require.resolve` is how a sibling optional dependency
// is located, and the package deliberately declares no `type`, so `.js`/`.cjs`
// here is CJS regardless of what any parent directory says.
'use strict';

const { spawn } = require('node:child_process');
const path = require('node:path');

// Keys are `${process.platform}-${process.arch}`, which is what npm itself
// matches `os`/`cpu` against, so this table and the published packages cannot
// disagree about what a platform is called.
const PACKAGES = {
  'darwin-arm64': '@wnzzer/agentdock-darwin-arm64',
  'darwin-x64': '@wnzzer/agentdock-darwin-x64',
  'linux-x64': '@wnzzer/agentdock-linux-x64',
  'linux-arm64': '@wnzzer/agentdock-linux-arm64',
  'win32-x64': '@wnzzer/agentdock-win32-x64',
};

const BINARY = process.platform === 'win32' ? 'agentdock-server.exe' : 'agentdock-server';

function fail(message) {
  process.stderr.write(`agentdock: ${message}\n`);
  process.exit(1);
}

function locate() {
  const key = `${process.platform}-${process.arch}`;
  const pkg = PACKAGES[key];
  if (!pkg) {
    fail(
      `no prebuilt binary for ${key}.\n` +
        `  Supported: ${Object.keys(PACKAGES).join(', ')}.\n` +
        '  Build from source instead: https://github.com/wnzzer/agentdock',
    );
  }
  // Resolve the manifest rather than the binary: a package.json is resolvable
  // whatever the file layout, and the error it throws names the missing
  // package instead of a path inside it.
  let manifest;
  try {
    manifest = require.resolve(`${pkg}/package.json`);
  } catch {
    fail(
      `${pkg} is not installed.\n` +
        '  It is an optional dependency, so npm skips it silently when the\n' +
        '  install ran on a different platform or with a lockfile from one.\n' +
        `  Reinstall on this machine, or add it directly: npm i ${pkg}`,
    );
  }
  return path.join(path.dirname(manifest), 'bin', BINARY);
}

const child = spawn(locate(), process.argv.slice(2), { stdio: 'inherit' });

// A terminal delivers Ctrl+C to the whole foreground group, so the server
// already sees it. Forwarding covers the other caller — a process manager
// signalling this pid alone, which would otherwise leave the server orphaned.
const FORWARDED = ['SIGINT', 'SIGTERM', 'SIGHUP', 'SIGQUIT'];
for (const signal of FORWARDED) {
  process.on(signal, () => {
    if (child.exitCode === null && child.signalCode === null) child.kill(signal);
  });
}

child.on('error', (error) => fail(`could not start the server: ${error.message}`));
child.on('exit', (code, signal) => {
  // Report a signalled death as a shell does, so `$?` still distinguishes
  // "killed" from "exited with that number".
  process.exit(signal ? 128 + (require('node:os').constants.signals[signal] ?? 0) : (code ?? 0));
});
