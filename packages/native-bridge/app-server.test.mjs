import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AppServerCancelled, AppServerClient, AppServerError, AppServerTimeout, NOT_STARTED } from './app-server.mjs';

// A stand-in app-server: a real child process speaking JSONL on stdio.
const FIXTURE = `
import { createInterface } from 'node:readline';
process.stdout.write('not json: a warning some clients print\\n');
let asked;
const send = message => process.stdout.write(JSON.stringify(message) + '\\n');
for await (const line of createInterface({ input: process.stdin })) {
  const message = JSON.parse(line);
  if (message.method === 'echo') send({ id: message.id, result: message.params });
  else if (message.method === 'fail') send({ id: message.id, error: { code: -32601, message: 'no such method' } });
  else if (message.method === 'silent') {}
  else if (message.method === 'exit') process.exit(0);
  else if (message.method === 'ask') { asked = message.id; send({ id: 'server-1', method: 'account/chatgptAuthTokens/refresh', params: {} }); }
  else if (message.id === 'server-1') {
    send({ method: 'account/login/completed', params: { loginId: 'login-1' } });
    send({ id: asked, result: { refusal: message.error } });
  }
}
`;

async function fixture(t, options = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'agentdock-app-server-'));
  const script = join(directory, 'server.mjs');
  await writeFile(script, FIXTURE);
  const closes = [], notes = [];
  const client = new AppServerClient(process.execPath, [script], { onClose: reason => closes.push(reason), onNotification: note => notes.push(note), ...options });
  t.after(async () => { await client.close(); await rm(directory, { recursive: true, force: true }); });
  return { client, closes, notes };
}

test('answers are matched to their requests, and lines that are not JSON are passed over', async t => {
  const { client } = await fixture(t);
  const [first, second] = await Promise.all([client.request('echo', { n: 1 }), client.request('echo', { n: 2 })]);
  assert.deepEqual([first, second], [{ n: 1 }, { n: 2 }]);
});

test('a JSON-RPC error keeps the code the server stated', async t => {
  const { client } = await fixture(t);
  await assert.rejects(client.request('fail'), error => error instanceof AppServerError && error.code === -32601);
});

test('a request from the server is refused, and its notifications still arrive', async t => {
  const { client, notes } = await fixture(t);
  const answer = await client.request('ask');
  assert.equal(answer.refusal.code, -32601);
  assert.deepEqual(notes.map(note => note.method), ['account/login/completed']);
});

test('timeouts and cancellation fail only the request they belong to', async t => {
  const { client } = await fixture(t);
  await assert.rejects(client.request('silent', {}, { timeout: 50 }), AppServerTimeout);
  const controller = new AbortController();
  const pending = client.request('silent', {}, { signal: controller.signal });
  controller.abort();
  await assert.rejects(pending, AppServerCancelled);
  assert.deepEqual(await client.request('echo', { still: 'open' }), { still: 'open' });
});

test('the server exiting fails what is waiting and reports why, once', async t => {
  const { client, closes } = await fixture(t);
  const waiting = client.request('silent');
  client.notify('exit');
  await assert.rejects(waiting, /exited/);
  assert.deepEqual(closes, ['Codex app-server exited.']);
  await assert.rejects(client.request('echo'), /closed/);
});

test('a program that cannot start is told apart from one that exited', async () => {
  const reason = await new Promise(resolve => {
    new AppServerClient(join(tmpdir(), 'agentdock-no-such-codex'), ['app-server'], { onClose: resolve });
  });
  assert.equal(reason, NOT_STARTED);
});
