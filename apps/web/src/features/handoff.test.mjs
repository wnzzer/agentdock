import test from 'node:test';
import assert from 'node:assert/strict';
import { handoffTranscript, HANDOFF_TRANSCRIPT_BYTES } from './handoff.ts';

const labels = { user: 'You', assistant: 'Claude Code', omitted: 'earlier messages omitted' };
const bytes = text => new TextEncoder().encode(text).byteLength;

test('a short conversation is carried over whole, in order, with who said what', () => {
  const { text, omitted } = handoffTranscript([
    { role: 'user', text: 'Rename the helper.' },
    { role: 'assistant', text: 'Renamed it in three files.' },
    { role: 'user', text: '   ' },
  ], labels);
  assert.equal(omitted, false);
  assert.equal(text, 'You:\nRename the helper.\n\nClaude Code:\nRenamed it in three files.');
});

// The server refuses a message over 32768 bytes, and Chinese text is three
// bytes a character, so the budget is counted in bytes rather than characters.
test('a long conversation keeps its latest turns and says the start was dropped', () => {
  const messages = Array.from({ length: 400 }, (_, index) => ({ role: index % 2 ? 'assistant' : 'user', text: `第${index}轮：` + '这是一段比较长的中文内容。'.repeat(8) }));
  const { text, omitted } = handoffTranscript(messages, labels);
  assert.equal(omitted, true);
  assert.ok(text.startsWith('(earlier messages omitted)'));
  assert.ok(text.includes('第399轮'));
  assert.ok(!text.includes('第0轮'));
  assert.ok(bytes(text) <= HANDOFF_TRANSCRIPT_BYTES + 64);
  assert.ok(bytes(text) + 2048 < 32768);
});

test('one message larger than the whole budget keeps its end, cut between characters', () => {
  const huge = '头' + '中'.repeat(20000) + '尾';
  const { text, omitted } = handoffTranscript([{ role: 'assistant', text: huge }], labels, 3000);
  assert.equal(omitted, true);
  assert.ok(text.endsWith('尾'));
  assert.ok(!text.includes('头'));
  assert.ok(!text.includes('�'));
  assert.ok(bytes(text) <= 3000 + 64);
});
