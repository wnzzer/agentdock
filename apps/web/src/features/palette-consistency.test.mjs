import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { TAB_ICON_PALETTE } from './tab-icons.ts';

const css = file => {
  const source = readFileSync(new URL(file, import.meta.url), 'utf8');
  const style = source.match(/<style scoped>([\s\S]*?)<\/style>/)?.[1];
  assert.ok(style, `${file} has scoped styles`);
  return style;
};

test('the existing workspace palette remains the source for new conversation and account surfaces', () => {
  const shell = readFileSync(new URL('../styles.css', import.meta.url), 'utf8');
  for (const token of ['--teal: #0c8376', '--teal-soft: #e7f4f0', '--violet: #7760b5', '--border: #e4e9ed', '--muted: #81909d', '--surface: #ffffff']) assert.ok(shell.includes(token));
  for (const file of ['./ChatSessionPane.vue', './AccountsDialog.vue', './ChatPreview.vue']) {
    const style = css(file);
    for (const token of ['var(--teal)', 'var(--teal-soft)', 'var(--border)', 'var(--muted)', 'var(--surface)']) assert.ok(style.includes(token), `${file} reuses ${token}`);
    assert.doesNotMatch(style, /#(?:fcfbf7|fffefb|f2f3eb|728b5f|66864f|748762)\b/i, `${file} does not introduce a separate cream/olive theme`);
  }
});

test('provider orange and purple stay consistent with the existing colored tabs', () => {
  for (const file of ['./ChatSessionPane.vue', './AccountsDialog.vue', './ChatPreview.vue']) {
    const style = css(file).toLowerCase();
    for (const provider of ['claude_code', 'codex']) {
      assert.ok(style.includes(TAB_ICON_PALETTE[provider].color.toLowerCase()), `${file} preserves ${provider} color`);
      assert.ok(style.includes(TAB_ICON_PALETTE[provider].background.toLowerCase()), `${file} preserves ${provider} background`);
    }
  }
});

test('chat retains mobile sizing and semantic warning/error colors within the original light palette', () => {
  const style = css('./ChatSessionPane.vue');
  assert.match(style, /\.chat-pane\{[^}]*background:var\(--surface\)/);
  assert.match(style, /\.chat-send\{[^}]*background:var\(--teal\)/);
  assert.match(style, /\.chat-composer\{[^}]*background:var\(--surface\)/);
  assert.ok(style.includes('min-height:44px'));
  assert.ok(style.includes('font-size:16px'));
  assert.ok(style.includes('safe-area-inset-bottom'));
  assert.ok(style.includes('#fffaee'));
  // The semantic error colour is unchanged; it is now named rather than repeated.
  assert.ok(style.includes('var(--danger-ink)'));
});
