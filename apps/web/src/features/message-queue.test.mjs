import test from 'node:test';
import assert from 'node:assert/strict';
import { messageQueue } from './message-queue.ts';

test('queued messages go out in order and each session keeps its own', () => {
  messageQueue.push('a', { id: '1', text: 'first' });
  messageQueue.push('a', { id: '2', text: 'second' });
  messageQueue.push('b', { id: '3', text: 'elsewhere' });
  assert.equal(messageQueue.remove('a', '2').text, 'second');
  assert.equal(messageQueue.shift('a').text, 'first');
  assert.equal(messageQueue.shift('a'), undefined);
  assert.equal(messageQueue.get('b').length, 1);
});
