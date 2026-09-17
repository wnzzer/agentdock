import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, rm, readFile, writeFile, readdir, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const read = (file) => readFile(path.join(here, file), 'utf8');

/** Build the packages from stand-in tarballs shaped like the release ones. */
async function build(version, targets) {
  const dir = await mkdtemp(path.join(tmpdir(), 'ad-npm-test-'));
  const tarballs = path.join(dir, 'tarballs');
  await mkdir(tarballs, { recursive: true });
  for (const triple of targets) {
    const name = `agentdock-${version}-${triple}`;
    const staged = path.join(dir, 'stage', name);
    await mkdir(staged, { recursive: true });
    await writeFile(path.join(staged, 'agentdock-server'), `#!/bin/sh\necho ${triple}\n`);
    await writeFile(path.join(staged, 'README.md'), '');
    execFileSync('tar', ['czf', path.join(tarballs, `${name}.tar.gz`), '-C', path.join(dir, 'stage'), name]);
  }
  const out = path.join(dir, 'out');
  execFileSync('node', [path.join(here, 'build-packages.mjs'), version, tarballs, out], { stdio: 'pipe' });
  return { dir, out };
}

// The shim maps a platform to a package name; the build script decides which
// packages exist. Nothing links them at runtime, so a target added to one and
// not the other publishes a release that simply cannot resolve on that machine
// — and only on that machine, which is the kind of gap CI never sees.
test('the shim and the build script agree on the set of platforms', async () => {
  const shim = await read('shim.cjs');
  const builder = await read('build-packages.mjs');

  const shimKeys = [...shim.matchAll(/'([a-z0-9]+-[a-z0-9]+)':\s*'@wnzzer\/agentdock-([a-z0-9-]+)'/g)];
  assert.ok(shimKeys.length >= 4, 'shim declares a platform table');
  for (const [, key, pkgSuffix] of shimKeys) {
    assert.equal(key, pkgSuffix, `shim maps ${key} to a package named for a different platform`);
  }

  const builderKeys = [...builder.matchAll(/key:\s*'([a-z0-9-]+)'/g)].map(([, key]) => key);
  assert.deepEqual(
    new Set(shimKeys.map(([, key]) => key)),
    new Set(builderKeys),
    'shim platform table and build targets have drifted apart',
  );
});

test('a built release installs as one coherent set', async (t) => {
  const version = '9.9.9';
  const triples = [
    'aarch64-apple-darwin',
    'x86_64-apple-darwin',
    'x86_64-unknown-linux-musl',
    'aarch64-unknown-linux-musl',
  ];
  const { dir, out } = await build(version, triples);
  t.after(() => rm(dir, { recursive: true, force: true }));

  const manifest = async (pkg) => JSON.parse(await readFile(path.join(out, pkg, 'package.json'), 'utf8'));
  const main = await manifest('agentdock');
  const order = (await readFile(path.join(out, 'publish-order.txt'), 'utf8')).trim().split('\n');

  // An optional dependency npm cannot find is skipped in silence, so a name or
  // version that does not match a package actually built degrades into the
  // "is not installed" path for every user rather than failing the publish.
  const built = (await readdir(out, { withFileTypes: true }))
    .filter((entry) => entry.isDirectory() && entry.name !== 'agentdock')
    .map((entry) => `@wnzzer/${entry.name}`);
  assert.deepEqual(new Set(Object.keys(main.optionalDependencies)), new Set(built));
  for (const [name, range] of Object.entries(main.optionalDependencies)) {
    assert.equal(range, version, `${name} must be pinned exactly, not to a range`);
  }

  // npm reads os/cpu before fetching. Getting them wrong either starves a
  // supported machine or hands it a binary for another architecture.
  for (const [platform, arch] of [
    ['darwin', 'arm64'],
    ['darwin', 'x64'],
    ['linux', 'x64'],
    ['linux', 'arm64'],
  ]) {
    const pkg = await manifest(`agentdock-${platform}-${arch}`);
    assert.deepEqual(pkg.os, [platform]);
    assert.deepEqual(pkg.cpu, [arch]);
    assert.equal(pkg.version, version);
    // The main package must stay small and carry no binary of its own.
    assert.ok(!('optionalDependencies' in pkg));
  }

  // The registry must know every dependency before the package that names them.
  assert.equal(order.at(-1), 'agentdock', 'the main package publishes last');
  assert.equal(new Set(order).size, order.length, 'no package is published twice');
  assert.equal(order.length, built.length + 1);

  // The binary embeds the web bundle, which embeds MIT-licensed brand vectors.
  // Publishing is redistribution, and that licence requires the notice to come
  // along; shipping only our own LICENSE would leave the obligation unmet.
  for (const pkg of ['agentdock', 'agentdock-darwin-arm64']) {
    const files = await readdir(path.join(out, pkg));
    assert.ok(files.includes('THIRD-PARTY-NOTICES.md'), `${pkg} omits the third-party notice`);
    assert.ok(files.includes('LICENSE'));
    assert.deepEqual(
      (await manifest(pkg)).files.filter((entry) => entry.endsWith('.md') || entry === 'LICENSE').sort(),
      pkg === 'agentdock' ? ['LICENSE', 'README.md', 'THIRD-PARTY-NOTICES.md'] : ['LICENSE', 'THIRD-PARTY-NOTICES.md'],
      `${pkg} would not actually publish the files on disk`,
    );
  }

  // An un-executable binary installs fine and fails at first use.
  const mode = await stat(path.join(out, 'agentdock-darwin-arm64', 'bin', 'agentdock-server'));
  assert.equal(mode.mode & 0o111, 0o111, 'the binary must be executable for all');

  // The path the shim computes from the resolved manifest.
  assert.ok((await read('shim.cjs')).includes("path.join(path.dirname(manifest), 'bin', BINARY)"));
  assert.equal(main.bin.agentdock, 'bin/agentdock.js');
});

// npm inherits stdin. A loop fed by `done < list` hands it the remaining
// package names, so one npm subcommand that reads stdin ends the loop early and
// publishes a subset — with a zero exit status, since every command in it
// succeeded. Verified against a stub npm: the piped form published 1 of 5.
test('the publish loop does not feed the package list to npm on stdin', async () => {
  const workflow = await readFile(path.join(root, '.github/workflows/release.yml'), 'utf8');
  const step = workflow.slice(workflow.indexOf('name: Publish'));
  assert.doesNotMatch(step, /done\s*<\s*dist-npm\/publish-order\.txt/, 'list is piped into the loop');
  assert.match(step, /packages=\$\(cat dist-npm\/publish-order\.txt\)/, 'list must be read before looping');
  // npm reads an argument shaped like owner/repo as a GitHub shorthand, so
  // `npm publish dist-npm/agentdock-darwin-arm64` fetches over ssh and dies on
  // a public key rather than publishing the directory sitting right there.
  assert.match(step, /npm publish "\.\/dist-npm\//, 'the publish path needs a ./ prefix to stay a path');
  // A dry run skips authentication entirely, so the credential is the one part
  // it cannot cover unless the job checks it outright.
  assert.match(step, /npm whoami/, 'the job must prove its credential in dry runs too');
  for (const call of step.match(/npm (view|publish)[^\n]*/g) ?? []) {
    assert.match(call, /<\/dev\/null/, `npm call must not inherit stdin: ${call.trim()}`);
  }
});

test('the published version always matches the workspace it was built from', async () => {
  const cargo = await readFile(path.join(root, 'Cargo.toml'), 'utf8');
  const version = cargo.match(/^version = "([^"]+)"/m)[1];
  const workflow = await readFile(path.join(root, '.github/workflows/release.yml'), 'utf8');
  // The job derives the version from Cargo.toml and refuses a tag that disagrees,
  // so a mistyped tag cannot put a wrong version on the registry permanently.
  assert.match(workflow, /does not match Cargo\.toml version/);
  // Publishing cannot be undone, so the job must be exercisable without doing
  // it: anything that is not a tag builds the packages and passes --dry-run.
  assert.match(workflow, /GITHUB_REF_TYPE.*=.*"tag"|\$\{GITHUB_REF_TYPE\}" = "tag"/);
  assert.match(workflow, /dry_run=--dry-run/);
  assert.match(workflow, /npm publish "\.\/dist-npm\/\$\{pkg\}" --access public \$DRY_RUN/);
  assert.ok(/^\d+\.\d+\.\d+/.test(version), `Cargo version ${version} is not publishable as-is`);
});
