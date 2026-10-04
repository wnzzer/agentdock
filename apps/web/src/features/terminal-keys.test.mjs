import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { arrowSequence, repeatArrow, selectionPresses, isTap, ctrlSequence, shiftSequence, tabSequence, ENTER_SEQUENCE, ESCAPE_SEQUENCE, TAB_SEQUENCE, BACKTAB_SEQUENCE } from './terminal-keys.ts';

test('arrow keys use the encoding the terminal state asks for', () => {
  // Outside application cursor mode the CSI form is what a shell expects.
  assert.equal(arrowSequence('up', false), '[A');
  assert.equal(arrowSequence('down', false), '[B');
  // A full-screen program that enabled DECCKM expects the SS3 form instead.
  assert.equal(arrowSequence('up', true), 'OA');
  assert.equal(arrowSequence('down', true), 'OB');
});

test('a modified arrow uses the CSI parameter form in either cursor mode', () => {
  assert.equal(arrowSequence('left', false, { ctrl: true }), '\x1b[1;5D');
  assert.equal(arrowSequence('left', true, { ctrl: true }), '\x1b[1;5D');
  assert.equal(arrowSequence('up', true, { shift: true }), '\x1b[1;2A');
  assert.equal(arrowSequence('right', false, { shift: true, ctrl: true }), '\x1b[1;6C');
  // No modifier armed is the plain key.
  assert.equal(arrowSequence('up', true, {}), '\x1bOA');
});

test('shift turns tab into shift+tab and a typed letter into its capital', () => {
  assert.equal(tabSequence(), TAB_SEQUENCE);
  assert.equal(tabSequence({ shift: true }), BACKTAB_SEQUENCE);
  assert.equal(shiftSequence('a'), 'A');
  // A word from an input method passes through unchanged.
  assert.equal(shiftSequence('hello'), 'hello');
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
  for (const label of ['Escape', 'Ctrl', 'Shift', 'Tab', 'Arrow left', 'Arrow up', 'Arrow down', 'Arrow right', 'Enter', 'Paste', 'Keyboard']) assert.ok(source.includes(`t('${label}')`), `${label} control is present`);
  // Shift+Tab is Shift armed and then Tab, not a second Tab-looking button.
  assert.doesNotMatch(source, />⇧Tab</);
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
  // Windows checks the source out with CRLF line endings.
  const tap = source.match(/function gestureEnd\([\s\S]*?\r?\n\}\r?\n/)?.[0] ?? '';
  assert.ok(tap.includes('selectionPresses('), 'the tap handler is found');
  assert.doesNotMatch(tap, /buffer\.active\.type/);
  assert.match(source, /!tapSelect\.value/);
  assert.match(source, /selectionPresses\(/);
  assert.ok(source.includes("t('Tap a row to select it')"), 'the mode is switchable from the bar');
  assert.match(source, /:aria-pressed="tapSelect"/, 'and says whether it is on');
});

test('ctrl is a one-shot modifier on what the soft keyboard types', () => {
  const source = readFileSync(new URL('../components/TerminalPane.vue', import.meta.url), 'utf8');
  // It rewrites the terminal's own input, so the soft keyboard supplies the
  // letter, and it releases after one key so it cannot stick on unnoticed.
  assert.match(source, /ctrlArmed\.value\) \{ ctrlArmed\.value = false; data = ctrlSequence\(data\)/);
  assert.match(source, /:aria-pressed="ctrlArmed"/);
});

test('paste reads the clipboard where it may and falls back to a field where it may not', () => {
  const source = readFileSync(new URL('../components/TerminalPane.vue', import.meta.url), 'utf8');
  // The Clipboard API exists only in a secure context, and AgentDock is often
  // reached over plain http on a private network; there the person pastes
  // into an ordinary field, which every mobile browser supports.
  assert.match(source, /navigator\.clipboard\?\.readText/);
  assert.match(source, /class="terminal-paste"/);
  // terminal.paste honours bracketed paste mode, so a multi-line paste is not
  // run line by line as if each newline were Enter.
  assert.match(source, /terminal\?\.paste\(/);
});
