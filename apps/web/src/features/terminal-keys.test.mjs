import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { arrowSequence, repeatArrow, selectionPresses, isTap, ENTER_SEQUENCE } from './terminal-keys.ts';

test('arrow keys use the encoding the terminal state asks for', () => {
  // Outside application cursor mode the CSI form is what a shell expects.
  assert.equal(arrowSequence('up', false), '[A');
  assert.equal(arrowSequence('down', false), '[B');
  // A full-screen program that enabled DECCKM expects the SS3 form instead.
  assert.equal(arrowSequence('up', true), 'OA');
  assert.equal(arrowSequence('down', true), 'OB');
});

test('enter sends a carriage return, which is what accepts a prompt', () => {
  assert.equal(ENTER_SEQUENCE, '\r');
});

test('tapping a row presses the arrow that many times, in the right direction', () => {
  assert.deepEqual(selectionPresses(4, 7), { key: 'down', count: 3 });
  assert.deepEqual(selectionPresses(7, 4), { key: 'up', count: 3 });
  // Tapping the row already highlighted moves nothing.
  assert.equal(selectionPresses(5, 5), undefined);
  assert.equal(selectionPresses(0, 0), undefined);
});

test('a mistaken tap costs a bounded number of keystrokes', () => {
  // An unbounded count would let one stray tap in a program that does not keep
  // its cursor on the highlighted row send hundreds of keystrokes.
  assert.deepEqual(selectionPresses(0, 500, 40), { key: 'down', count: 40 });
  assert.deepEqual(selectionPresses(500, 0, 40), { key: 'up', count: 40 });
});

test('a non-integer row is not a selection target', () => {
  assert.equal(selectionPresses(1.5, 3), undefined);
  assert.equal(selectionPresses(1, Number.NaN), undefined);
});

test('repeating an arrow keeps the same encoding throughout', () => {
  assert.equal(repeatArrow('down', 3, false), '[B[B[B');
  assert.equal(repeatArrow('up', 2, true), 'OAOA');
  assert.equal(repeatArrow('up', 0, false), '');
  // A negative count must not throw or produce garbage.
  assert.equal(repeatArrow('up', -5, false), '');
});

test('a tap is distinguished from a scroll, a drag and a text selection', () => {
  assert.equal(isTap({ movedPx: 0, elapsedMs: 60, hasSelection: false }), true);
  assert.equal(isTap({ movedPx: 8, elapsedMs: 300, hasSelection: false }), true);
  // A scroll drag.
  assert.equal(isTap({ movedPx: 60, elapsedMs: 120, hasSelection: false }), false);
  // A long press, which is how a touch device selects text.
  assert.equal(isTap({ movedPx: 2, elapsedMs: 900, hasSelection: false }), false);
  // A fingertip that stayed still but left a selection behind.
  assert.equal(isTap({ movedPx: 1, elapsedMs: 100, hasSelection: true }), false);
});

test('the terminal surface exposes the controls and keeps them out of the scroll area', () => {
  const source = readFileSync(new URL('../components/TerminalPane.vue', import.meta.url), 'utf8');
  // Without these the feature does not exist, however correct the helpers are.
  assert.match(source, /terminal-keys/);
  assert.match(source, /class="terminal-keys"/);
  for (const label of ['Arrow up', 'Arrow down', 'Enter', 'Keyboard']) assert.ok(source.includes(`t('${label}')`), `${label} control is present`);
  const styles = readFileSync(new URL('./workflows.css', import.meta.url), 'utf8');
  // Floating, not a row of the terminal: a full-screen program has been told
  // how tall the screen is, and taking a row back would misalign its redraws.
  assert.match(styles, /\.terminal-keys \{[^}]*position: absolute/);
  assert.match(styles, /\.native-terminal \{[^}]*position: relative/);
});

test('tapping a row sends arrows only while that gesture is explicitly armed', () => {
  const source = readFileSync(new URL('../components/TerminalPane.vue', import.meta.url), 'utf8');
  // A buffer-type gate was tried first and is wrong: Claude Code draws its
  // menus with Ink, in the ordinary buffer, so "alternate screen" excluded the
  // one case this exists for while a shell prompt looks identical to it. The
  // terminal cannot tell a menu from a prompt, so the gesture is armed by hand.
  assert.doesNotMatch(source, /buffer\.active\.type/);
  assert.match(source, /!tapSelect\.value/);
  assert.match(source, /selectionPresses\(/);
  assert.ok(source.includes("t('Tap a row to select it')"), 'the mode is switchable from the bar');
  assert.match(source, /:aria-pressed="tapSelect"/, 'and says whether it is on');
});
