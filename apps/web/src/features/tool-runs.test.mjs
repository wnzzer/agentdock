import test from 'node:test';
import assert from 'node:assert/strict';
import { toolRuns, toolRunNames, toolRunStatus } from './tool-runs.ts';

const tool = (id, name, status = 'completed') => ({ type: 'tool', id, name, status, text: '' });
const say = (id, text) => ({ type: 'message', id, role: 'assistant', text });

test('consecutive tool calls fold into one run, and anything between them ends it', () => {
  const view = toolRuns([say('a', 'Looking'), tool('1', 'Bash'), tool('2', 'Bash'), tool('3', 'Read'), say('b', 'Found it'), tool('4', 'Edit')]);
  assert.deepEqual(view.map(item => item.type === 'tool_run' ? item.tools.map(t => t.id) : item.id), ['a', ['1', '2', '3'], 'b', ['4']]);
  assert.equal(toolRunNames(view[1]), 'Bash ×2 · Read');
});

test('a failed call is never folded behind a run that reads as fine', () => {
  const view = toolRuns([tool('1', 'Bash'), tool('2', 'Bash', 'failed'), tool('3', 'Bash')]);
  assert.deepEqual(view.map(run => run.tools.map(t => t.id)), [['1'], ['2'], ['3']]);
  assert.equal(toolRunStatus(view[1]), 'failed');
});

test('a run is running while any of its calls is', () => {
  const [run] = toolRuns([tool('1', 'Bash'), tool('2', 'Read', 'running')]);
  assert.equal(toolRunStatus(run), 'running');
});
