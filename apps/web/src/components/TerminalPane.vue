<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import Icon from "../features/Icon.vue";
import { useI18n } from "../i18n";
import { createSessionStream, TERMINAL_VIEW_ERROR } from "../features/session-stream";
import { onPageReturn } from "../features/page-return";
import type { SessionStreamState } from "../features/session-stream";
import { arrowSequence, BACKTAB_SEQUENCE, ctrlSequence, ENTER_SEQUENCE, ESCAPE_SEQUENCE, isTap, repeatArrow, selectionPresses, TAB_SEQUENCE, type ArrowKey } from "../features/terminal-keys";
const { t } = useI18n();

const props = withDefaults(defineProps<{ sessionId: string; dark?: boolean }>(), { dark: false });
const emit = defineEmits<{ exit: []; status: [connected: boolean] }>();
const host = ref<HTMLDivElement>();
const state = ref<SessionStreamState>("connecting");
const notice = ref("");
const nativeNotice = ref(false);
/**
 * On-screen keys, for a device whose keyboard has no arrows and cannot be
 * relied on to be visible at all. A physical keyboard already sends these, so
 * the controls appear only where the pointer is a fingertip.
 */
const touchDevice = ref(false);
/**
 * Tap a row to move a list selection onto it.
 *
 * Off by default and switched on explicitly, because the same tap means two
 * different things depending on what is running. In a menu it picks a row; at a
 * shell prompt the arrows it sends walk through command history instead, which
 * is not something a stray tap should do. The terminal cannot tell those apart
 * -- the client command this exists for draws its menu in the ordinary buffer,
 * exactly like a prompt -- so the person holding the phone says which it is.
 */
const tapSelect = ref(false);
/** One-shot Ctrl: the next character the soft keyboard types becomes a control code. */
const ctrlArmed = ref(false);
/**
 * Text waiting to be pasted, when the clipboard could not be read directly.
 * `undefined` means the paste field is closed.
 */
const pasteDraft = ref<string>();
const pasteField = ref<HTMLTextAreaElement>();
let terminal: Terminal | undefined;
let fit: FitAddon | undefined;
let observer: ResizeObserver | undefined;
let resizeFrame = 0, sized = false;
let disposed = false;
const subscriptions: Array<{ dispose: () => void }> = [];
const stream = createSessionStream({
  createSocket: url => new WebSocket(url),
  onState(next, message) { state.value = next; notice.value = message; nativeNotice.value = false; emit("status", next === "connected"); },
  onOpen() { sized = false; resize(); },
  onMessage(data) {
    if (data instanceof ArrayBuffer) {
      terminal?.write(new Uint8Array(data));
      // A size sent as the socket opens can reach the server before a reopened
      // session's process exists, leaving the shell at the size it started
      // with. Its first output proves the process is there; say the size again.
      if (!sized) { sized = true; resize(); }
      return;
    }
    if (typeof data !== "string") return;
    let control: { type: string; message?: string; code?: number };
    try {
      const parsed: unknown = JSON.parse(data);
      if (!parsed || typeof parsed !== "object" || typeof (parsed as { type?: unknown }).type !== "string") throw new Error("Invalid control message");
      control = parsed as typeof control;
    } catch { nativeNotice.value = false; notice.value = "An unsupported control message was received. Reconnect to refresh the view."; return; }
    const message = typeof control.message === "string" ? control.message : undefined;
    if (control.type === "exit") {
      stream.finish("Process exited. Start the session again to create a new process."); emit("exit");
    } else if (control.type === "gap") {
      nativeNotice.value = message !== undefined;
      notice.value = message ?? "The stream exceeded its buffer. Reconnect to restore available history; this does not restart the process.";
    } else if (control.type === "error") {
      stream.fail(message ?? "The session stream reported an error."); nativeNotice.value = message !== undefined;
    }
  },
});

function resize() {
  cancelAnimationFrame(resizeFrame);
  resizeFrame = requestAnimationFrame(() => {
    if (disposed || !terminal || !host.value || host.value.clientWidth < 30 || host.value.clientHeight < 30) return;
    try { fit?.fit(); } catch { return; }
    stream.send(JSON.stringify({ type: "resize", cols: terminal.cols, rows: terminal.rows }));
  });
}

/** Send exactly the bytes the physical key would, so the client sees a key. */
function sendKeys(sequence: string) {
  if (!sequence || !terminal) return;
  // A key on the bar is the key Ctrl was armed for, if any; it never combines,
  // so the modifier is released rather than left waiting for the keyboard.
  ctrlArmed.value = false;
  stream.send(new TextEncoder().encode(sequence));
}
function pressArrow(key: ArrowKey) {
  if (!terminal) return;
  sendKeys(arrowSequence(key, terminal.modes.applicationCursorKeysMode));
}
function pressEnter() { sendKeys(ENTER_SEQUENCE); }
function pressEscape() { sendKeys(ESCAPE_SEQUENCE); }
function pressTab(back: boolean) { sendKeys(back ? BACKTAB_SEQUENCE : TAB_SEQUENCE); }
/**
 * Paste from the clipboard.
 *
 * The Clipboard API is available only in a secure context, and a phone often
 * reaches AgentDock over plain http on a private network. Where it cannot be
 * read -- or the browser refuses -- an ordinary field opens instead, which
 * every mobile browser can paste into with a long press.
 */
async function pressPaste() {
  ctrlArmed.value = false;
  try {
    const text = await navigator.clipboard?.readText?.();
    if (text) { terminal?.paste(text); return; }
  } catch { /* Not permitted here; fall back to the field. */ }
  pasteDraft.value = "";
  await nextTick(); pasteField.value?.focus();
}
function sendPaste() {
  const text = pasteDraft.value;
  pasteDraft.value = undefined;
  if (text) terminal?.paste(text);
  terminal?.focus();
}
function cancelPaste() { pasteDraft.value = undefined; terminal?.focus(); }
/**
 * Bring up the on-screen keyboard by focusing the terminal's own input target.
 * A mobile browser only opens it from inside a real gesture, which is why this
 * runs on click and why the other controls refuse focus instead of taking it.
 */
function showKeyboard() { terminal?.focus(); }
/** Keep the caret in the terminal so tapping a control never closes the keyboard. */
function keepFocus(event: Event) { event.preventDefault(); }

let gesture: { x: number; y: number; at: number } | undefined;
function gestureStart(event: PointerEvent) { gesture = { x: event.clientX, y: event.clientY, at: Date.now() }; }
/**
 * Tap a row to move the selection to it.
 *
 * The distance from the cursor row to the tapped row is a number of arrow
 * presses, which holds because a list keeps the cursor on the row it has
 * highlighted -- verified against the client's own menu, where the cursor row
 * and the marked row are the same row.
 */
function gestureEnd(event: PointerEvent) {
  const start = gesture;
  gesture = undefined;
  if (!start || !tapSelect.value || !terminal || !host.value) return;
  const moved = Math.hypot(event.clientX - start.x, event.clientY - start.y);
  if (!isTap({ movedPx: moved, elapsedMs: Date.now() - start.at, hasSelection: terminal.hasSelection() })) return;
  const viewport = host.value.querySelector(".xterm-screen") ?? host.value;
  const bounds = viewport.getBoundingClientRect();
  if (bounds.height < 1 || terminal.rows < 1) return;
  const row = Math.floor((event.clientY - bounds.top) / (bounds.height / terminal.rows));
  if (row < 0 || row >= terminal.rows) return;
  const move = selectionPresses(terminal.buffer.active.cursorY, row);
  if (move) sendKeys(repeatArrow(move.key, move.count, terminal.modes.applicationCursorKeysMode));
}

function connect() {
  if (disposed || !props.sessionId) return;
  try {
    initializeTerminal();
    terminal!.reset();
    const url = new URL(`/api/sessions/${encodeURIComponent(props.sessionId)}/pty/ws`, window.location.href);
    url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    url.searchParams.set("cols", String(terminal!.cols));
    url.searchParams.set("rows", String(terminal!.rows));
    stream.connect(url.toString()); resize();
  } catch { stream.fail(TERMINAL_VIEW_ERROR); cleanupTerminal(); }
}
defineExpose({ reconnect: connect });

function initializeTerminal() {
  if (terminal) return;
  if (!host.value) throw new Error("Terminal host is unavailable");
  terminal = new Terminal({
    cursorBlink: true, fontSize: 12, lineHeight: 1.22, scrollback: 5000,
    // A Nerd Font the viewer already has wins; the bundled symbols fill in the
    // prompt icons for everyone else, phones included.
    fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", "Symbols Nerd Font Mono", "AgentDock Symbols", monospace',
    theme: props.dark ? { background: "#17232d", foreground: "#dde6eb", cursor: "#63dac8" } : {
      background: "#ffffff", foreground: "#243343", cursor: "#0a8278", selectionBackground: "#ccebe6",
      black: "#243343", brightBlack: "#657380", red: "#b4374c", brightRed: "#c73f55", green: "#187953", brightGreen: "#098462",
      yellow: "#916100", brightYellow: "#9b6800", blue: "#3467af", brightBlue: "#3375c5", magenta: "#7856aa", brightMagenta: "#8965bc",
      cyan: "#087e80", brightCyan: "#058587", white: "#607080", brightWhite: "#384658",
    },
  });
  fit = new FitAddon(); terminal.loadAddon(fit); terminal.open(host.value);
  subscriptions.push(terminal.onData(data => {
    if (ctrlArmed.value) { ctrlArmed.value = false; data = ctrlSequence(data); }
    stream.send(new TextEncoder().encode(data));
  }));
  subscriptions.push(terminal.onBinary(data => { stream.send(Uint8Array.from(data, value => value.charCodeAt(0) & 255)); }));
  observer = new ResizeObserver(resize); observer.observe(host.value);
}
function cleanupTerminal() {
  cancelAnimationFrame(resizeFrame);
  observer?.disconnect(); observer = undefined;
  for (const subscription of subscriptions.splice(0)) try { subscription.dispose(); } catch { /* Best-effort cleanup after partial initialization. */ }
  try { terminal?.dispose(); } catch { /* A broken renderer should remain retryable. */ }
  terminal = undefined; fit = undefined; host.value?.replaceChildren();
}
onMounted(() => {
  // `any-pointer: coarse` rather than `pointer: coarse`: a laptop with a
  // touchscreen keeps its keyboard, and showing touch-only controls there would
  // be clutter over the terminal.
  try { touchDevice.value = window.matchMedia?.("(any-pointer: coarse)").matches === true; } catch { touchDevice.value = false; }
  connect();
});
/**
 * Coming back to the page re-attaches a view that dropped meanwhile, and one
 * that may only look attached after a long absence. Only the view: an exited
 * process stays exited, and nothing typed is replayed.
 */
let stopPageReturn = () => {};
onMounted(() => {
  stopPageReturn = onPageReturn(stale => {
    if (state.value === "disconnected" || state.value === "error" || (stale && state.value === "connected")) connect();
  });
});
watch(() => props.sessionId, async (_next, _previous, onCleanup) => {
  let current = true; onCleanup(() => { current = false; });
  stream.disconnect(); await nextTick(); if (current) connect();
});
onUnmounted(() => { stopPageReturn(); disposed = true; stream.dispose(); cleanupTerminal(); emit("status", false); });
</script>

<template>
  <div :class="['native-terminal', { dark }]">
    <div class="terminal-connection"><span :class="['state-dot', state]" /><span>{{ t(state === 'connected' ? 'Live · native client' : state) }}</span><button v-if="state === 'disconnected' || state === 'error'" class="text-button" :aria-label="t('Reconnect session stream')" @click="connect"><Icon name="refresh" :size="13" />{{ t('Reconnect') }}</button></div>
    <div v-if="notice" :class="state === 'error' ? 'inline-error' : 'inline-notice'" :role="state === 'error' ? 'alert' : 'status'">{{ nativeNotice ? notice : t(notice) }}</div>
    <div ref="host" class="terminal-host" :aria-label="t('Native session {id}', { id: sessionId })" @pointerdown="gestureStart" @pointerup="gestureEnd" @pointercancel="gesture = undefined" />
    <div v-if="touchDevice" class="terminal-keys" role="group" :aria-label="t('Terminal keys')">
      <button type="button" class="key-text" :aria-label="t('Escape')" :title="t('Escape')" @pointerdown="keepFocus" @click="pressEscape">Esc</button>
      <button type="button" class="key-text" :class="{ armed: ctrlArmed }" :aria-pressed="ctrlArmed" :aria-label="t('Ctrl')" :title="t('Ctrl')" @pointerdown="keepFocus" @click="ctrlArmed = !ctrlArmed">Ctrl</button>
      <button type="button" class="key-text" :aria-label="t('Tab')" :title="t('Tab')" @pointerdown="keepFocus" @click="pressTab(false)">Tab</button>
      <button type="button" class="key-text" :aria-label="t('Shift+Tab')" :title="t('Shift+Tab')" @pointerdown="keepFocus" @click="pressTab(true)">⇧Tab</button>
      <button type="button" :aria-label="t('Arrow left')" :title="t('Arrow left')" @pointerdown="keepFocus" @click="pressArrow('left')"><Icon name="chevron" :size="16" class="key-left" /></button>
      <button type="button" :aria-label="t('Arrow up')" :title="t('Arrow up')" @pointerdown="keepFocus" @click="pressArrow('up')"><Icon name="chevron" :size="16" class="key-up" /></button>
      <button type="button" :aria-label="t('Arrow down')" :title="t('Arrow down')" @pointerdown="keepFocus" @click="pressArrow('down')"><Icon name="chevron" :size="16" class="key-down" /></button>
      <button type="button" :aria-label="t('Arrow right')" :title="t('Arrow right')" @pointerdown="keepFocus" @click="pressArrow('right')"><Icon name="chevron" :size="16" /></button>
      <button type="button" :aria-label="t('Enter')" :title="t('Enter')" @pointerdown="keepFocus" @click="pressEnter"><Icon name="check" :size="16" /></button>
      <button type="button" :aria-label="t('Paste')" :title="t('Paste')" @click="pressPaste"><Icon name="clipboard" :size="16" /></button>
      <button type="button" :aria-label="t('Keyboard')" :title="t('Keyboard')" @click="showKeyboard"><Icon name="edit" :size="16" /></button>
      <button type="button" :class="{ armed: tapSelect }" :aria-pressed="tapSelect" :aria-label="t('Tap a row to select it')" :title="t('Tap a row to select it')" @pointerdown="keepFocus" @click="tapSelect = !tapSelect"><Icon name="locate" :size="16" /></button>
    </div>
    <form v-if="pasteDraft !== undefined" class="terminal-paste" @submit.prevent="sendPaste" @keydown.esc.prevent="cancelPaste">
      <textarea ref="pasteField" v-model="pasteDraft" :aria-label="t('Text to paste')" :placeholder="t('Long-press here to paste, then send it to the terminal')" />
      <div><button type="button" class="secondary-button" @click="cancelPaste">{{ t('Cancel') }}</button><button type="submit" class="primary-button" :disabled="!pasteDraft">{{ t('Send') }}</button></div>
    </form>
  </div>
</template>
