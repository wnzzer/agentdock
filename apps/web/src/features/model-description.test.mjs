import { test } from 'node:test';
import assert from 'node:assert/strict';
import { describeModel } from './model-description.ts';
import { translate } from '../i18n/index.ts';

const zh = (source, values) => translate(source, 'zh-CN', values);

test("a client's own words are translated phrase by phrase, and its names are left alone", () => {
  // Real lines from Claude Code. The prose is translated; the model's name and
  // the price it quotes are reproduced, because neither is ours to reword.
  assert.equal(
    describeModel('Sonnet 5 · Efficient for routine tasks · $2/$10 per Mtok', zh),
    'Sonnet 5 · 处理常规任务更高效 · $2/$10 / 每百万 token');
  assert.equal(
    describeModel('Opus 5 with 1M context · Best for everyday, complex tasks · $5/$25 per Mtok', zh),
    'Opus 5 with 1M context · 适合日常与复杂任务 · $5/$25 / 每百万 token');
  // A sentence with the client's own value in it keeps the value and translates
  // the sentence around it.
  assert.equal(
    describeModel('Use the default model (currently Opus 5 (1M context)) · $5/$25 per Mtok', zh),
    '使用默认模型（当前为 Opus 5 (1M context)） · $5/$25 / 每百万 token');
  assert.equal(describeModel('Haiku 4.5 · Fastest for quick answers', zh), 'Haiku 4.5 · 回答最快');
});

test('a line this dictionary has never seen is shown as the client wrote it', () => {
  const unknown = 'Sonnet 6 · Tuned for tool use in long sessions · $3/$15 per Mtok';
  assert.equal(describeModel(unknown, zh), 'Sonnet 6 · Tuned for tool use in long sessions · $3/$15 / 每百万 token');
  assert.equal(describeModel('Something entirely new', zh), 'Something entirely new');
  // English keeps every word the client chose, including the ones translated
  // elsewhere.
  const en = (source, values) => translate(source, 'en', values);
  assert.equal(describeModel('Sonnet 5 · Efficient for routine tasks · $2/$10 per Mtok', en),
    'Sonnet 5 · Efficient for routine tasks · $2/$10 per Mtok');
  assert.equal(describeModel('', zh), '');
});
