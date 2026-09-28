import test from 'node:test';
import assert from 'node:assert/strict';
import { ownStoredSecret, referencedSecret, secretNameFor } from './secret-names.ts';

test('profile names become secret names, never reusing a taken one', () => {
  assert.equal(secretNameFor('Work proxy', []), 'AGENTDOCK_SECRET_WORK_PROXY');
  assert.equal(secretNameFor('  deepseek-v3 (fast)', []), 'AGENTDOCK_SECRET_DEEPSEEK_V3_FAST');
  assert.equal(secretNameFor('中文端点', []), 'AGENTDOCK_SECRET_KEY');
  assert.equal(secretNameFor('work', ['AGENTDOCK_SECRET_WORK', 'AGENTDOCK_SECRET_WORK_2']), 'AGENTDOCK_SECRET_WORK_3');
});

test('references parse only in their exact form', () => {
  assert.equal(referencedSecret('env:AGENTDOCK_SECRET_A1'), 'AGENTDOCK_SECRET_A1');
  assert.equal(referencedSecret(' env:AGENTDOCK_SECRET_A1 '), 'AGENTDOCK_SECRET_A1');
  for (const bad of ['AGENTDOCK_SECRET_A', 'env:HOME', 'env:AGENTDOCK_SECRET_a', '', null, undefined]) assert.equal(referencedSecret(bad), undefined);
});

test('a profile reuses only a stored key that is its own alone', () => {
  const stored = [{ name: 'AGENTDOCK_SECRET_MINE', source: 'agentdock' }, { name: 'AGENTDOCK_SECRET_ENV', source: 'environment' }];
  assert.equal(ownStoredSecret('env:AGENTDOCK_SECRET_MINE', stored, []), 'AGENTDOCK_SECRET_MINE');
  assert.equal(ownStoredSecret('env:AGENTDOCK_SECRET_MINE', stored, ['env:AGENTDOCK_SECRET_MINE']), undefined);
  assert.equal(ownStoredSecret('env:AGENTDOCK_SECRET_ENV', stored, []), undefined);
  assert.equal(ownStoredSecret(null, stored, []), undefined);
});
