import { test } from 'node:test';
import assert from 'node:assert/strict';
import { filterModels, mergeModels, resolveModel } from './model-choices.ts';

// An endpoint behind a gateway that serves models under IDs nobody would type.
const endpoint = [
  { id: 'claude-fable-5-dd-lacol-b9-5.3newq', name: 'qwen3.5-9b-local' },
  { id: 'claude-fable-5-dd-3.5-MLG/gro-iaz', name: 'zai-org/GLM-5.3' },
  // Two models that share a name cannot be told apart by it.
  { id: 'claude-fable-5-dd-a', name: 'twin' },
  { id: 'claude-fable-5-dd-b', name: 'twin' },
];

test("a session's menu lists the endpoint's models, named by the endpoint", () => {
  const client = [
    { id: 'default', name: 'Default (recommended)', isDefault: true },
    // The client guessed this name from the ID's prefix.
    { id: 'claude-fable-5-dd-lacol-b9-5.3newq', name: 'Fable 5' },
    { id: 'sonnet', name: 'Sonnet' },
  ];
  const merged = mergeModels(client, endpoint);
  assert.deepEqual(merged.map(entry => entry.id), ['default', 'claude-fable-5-dd-lacol-b9-5.3newq', 'sonnet', 'claude-fable-5-dd-3.5-MLG/gro-iaz', 'claude-fable-5-dd-a', 'claude-fable-5-dd-b']);
  assert.equal(merged[1].name, 'qwen3.5-9b-local', "the endpoint's name wins over the client's guess");
  assert.equal(merged[0].isDefault, true, "the client's own entries keep what they carry");
  assert.deepEqual(mergeModels(client, []), client, 'no endpoint catalog, the client list as it was');
});

test('a model typed by its name is sent by its ID', () => {
  assert.equal(resolveModel('qwen3.5-9b-local', endpoint), 'claude-fable-5-dd-lacol-b9-5.3newq');
  assert.equal(resolveModel('  QWEN3.5-9B-LOCAL ', endpoint), 'claude-fable-5-dd-lacol-b9-5.3newq', 'case and spaces do not matter');
  assert.equal(resolveModel('claude-fable-5-dd-3.5-MLG/gro-iaz', endpoint), 'claude-fable-5-dd-3.5-MLG/gro-iaz', 'an ID stays an ID');
  assert.equal(resolveModel('twin', endpoint), 'twin', 'an ambiguous name is not guessed at');
  assert.equal(resolveModel('claude-opus-5-5', endpoint), 'claude-opus-5-5', 'anything else is sent as typed');
  assert.equal(resolveModel('qwen3.5-9b-local', []), 'qwen3.5-9b-local', 'without a catalog nothing can be resolved');
});

test('the menu narrows to models whose name or ID matches', () => {
  assert.deepEqual(filterModels(endpoint, 'glm').map(entry => entry.name), ['zai-org/GLM-5.3']);
  assert.deepEqual(filterModels(endpoint, 'LACOL').map(entry => entry.name), ['qwen3.5-9b-local'], 'by ID too');
  assert.equal(filterModels(endpoint, '  ').length, endpoint.length);
});
