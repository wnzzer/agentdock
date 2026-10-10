<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import Icon from "../features/Icon.vue";
import { useI18n } from "../i18n";
import { createSessionStream, TERMINAL_VIEW_ERROR } from "../features/session-stream";
import { onPageReturn } from "../features/page-return";
import type { SessionStreamState } from "../features/session-stream";
import { arrowSequence, ctrlSequence, ENTER_SEQUENCE, ESCAPE_SEQUENCE, isTap, repeatArrow, selectionPresses, shiftSequence, tabSequence, type ArrowKey } from "../features/terminal-keys";
import { swipeAxis } from "../features/pane-swipe";
import { terminalDark as dark } from "../features/theme";
import { filesFrom, imageCaveat, isImage, pasteText, uploadName, uploadProblem, uploadToSession } from "../features/terminal-uploads";
import { showToast } from "../features/toasts";
const { t } = useI18n();

const props = defineProps<{ sessionId: string }>();
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
/** One-shot Shift: Tab becomes Shift+Tab, an arrow a Shift+arrow, a letter its capital. */
const shiftArmed = ref(false);
/** The less used keys fold behind ⋯ so the everyday ones fit one phone-wide row. */
const moreKeys = ref(false);
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
/**
 * Where a reconnect picks up: the process the view was showing and the last
 * output it drew. The server resumes from there when it still can, so a
 * dropped socket costs nothing on screen; otherwise it says so and the view is
 * redrawn from the replay. `framed` is whether this socket numbers its frames.
 */
let resume: { runtime: string; seq: number } | undefined;
let framed = false, redrawPending = false;
const stream = createSessionStream({
  createSocket: url => new WebSocket(url),
  reconnect: true,
  hidden: () => document.hidden,
  onState(next, message) { state.value = next; notice.value = message; nativeNotice.value = false; emit("status", next === "connected"); },
  // Until the server says whether it resumed, the old screen stays up rather
  // than blanking for the length of a handshake.
  onOpen() { sized = false; framed = false; redrawPending = true; resize(); },
  onMessage(data) {
    if (data instanceof ArrayBuffer) {
      if (framed) {
        if (data.byteLength < 8) return;
        resume = { runtime: resume?.runtime ?? "", seq: Number(new DataView(data).getBigUint64(0)) };
        terminal?.write(new Uint8Array(data, 8));
      } else {
        // A server that does not number its frames replays everything.
        if (redrawPending) { redrawPending = false; terminal?.reset(); }
        terminal?.write(new Uint8Array(data));
      }
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
    if (control.type === "stream") {
      const hello = control as { runtime?: unknown; resumed?: unknown };
      framed = true; redrawPending = false;
      if (hello.resumed !== true || typeof hello.runtime !== "string" || hello.runtime !== resume?.runtime) terminal?.reset();
      resume = typeof hello.runtime === "string" ? { runtime: hello.runtime, seq: hello.resumed === true && resume ? resume.seq : 0 } : undefined;
    } else if (control.type === "exit") {
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
  // A key on the bar is the key the modifiers were armed for, if any; they are
  // released rather than left waiting for the keyboard.
  ctrlArmed.value = false; shiftArmed.value = false;
  stream.send(new TextEncoder().encode(sequence));
}
function pressArrow(key: ArrowKey) {
  if (!terminal) return;
  sendKeys(arrowSequence(key, terminal.modes.applicationCursorKeysMode, { shift: shiftArmed.value, ctrl: ctrlArmed.value }));
}
function pressEnter() { sendKeys(ENTER_SEQUENCE); }
function pressEscape() { sendKeys(ESCAPE_SEQUENCE); }
function pressTab() { sendKeys(tabSequence({ shift: shiftArmed.value })); }
/** A slash on its own key: it opens the command menu of Claude Code, Codex and most TUIs, and a phone keyboard hides it a layer down. */
function pressSlash() { sendKeys("/"); }
/**
 * Paste from the clipboard.
 *
 * The Clipboard API is available only in a secure context, and a phone often
 * reaches AgentDock over plain http on a private network. Where it cannot be
 * read -- or the browser refuses -- an ordinary field opens instead, which
 * every mobile browser can paste into with a long press.
 */
async function pressPaste() {
  ctrlArmed.value = false; shiftArmed.value = false;
  try {
    // An image on the clipboard first, where the browser lets it be read.
    const items = await navigator.clipboard?.read?.().catch(() => undefined);
    const images: File[] = [];
    for (const item of items ?? []) {
      const type = item.types.find(kind => kind.startsWith("image/"));
      if (type) images.push(new File([await item.getType(type)], "image." + (type.split("/")[1] ?? "png"), { type }));
    }
    if (images.length) { void sendFiles(images); return; }
    const text = await navigator.clipboard?.readText?.();
    if (text) { terminal?.paste(text); return; }
  } catch { /* Not permitted here; fall back to the field. */ }
  pasteDraft.value = "";
  await nextTick(); pasteField.value?.focus();
}
/**
 * Files on their way into the terminal. Each shows as a chip while it uploads;
 * when a batch is in, the paths are pasted in the order the files came, and a
 * failed one stays as a chip to retry or dismiss, pasting nothing half-made.
 */
interface Upload { id: number; file: File; name: string; progress: number; failed?: string; retryable?: boolean; preview?: string; abort?: () => void }
const uploads = ref<Upload[]>([]);
const dragging = ref(false);
const fileInput = ref<HTMLInputElement>();
let uploadSeq = 0;
function dropUpload(id: number) {
  const entry = uploads.value.find(item => item.id === id);
  entry?.abort?.(); if (entry?.preview) URL.revokeObjectURL(entry.preview);
  uploads.value = uploads.value.filter(item => item.id !== id);
}
async function uploadOne(entry: Upload): Promise<string | undefined> {
  const problem = uploadProblem(entry.file);
  if (problem) { entry.failed = t(problem); entry.retryable = false; return undefined; }
  entry.failed = undefined; entry.progress = 0;
  const job = uploadToSession(props.sessionId, entry.file, entry.name, fraction => { entry.progress = fraction; });
  entry.abort = job.abort;
  try {
    const saved = await job.promise;
    const caveat = imageCaveat(saved.name);
    if (caveat) showToast(t(caveat), { tone: "warn" });
    return saved.absolute;
  } catch (cause) { entry.failed = cause instanceof Error ? t(cause.message) : t("Upload failed"); entry.retryable = true; return undefined; }
  finally { entry.abort = undefined; }
}
/** Upload files and paste their paths, as dragging them into a local terminal would. */
async function sendFiles(files: File[]) {
  if (!files.length) return;
  if (!terminal || state.value !== "connected") { showToast(t("Connect the terminal before adding files."), { tone: "warn" }); return; }
  const batch = files.map(file => {
    const name = uploadName(file);
    const entry: Upload = { id: ++uploadSeq, file, name, progress: 0, preview: isImage(name, file.type) ? URL.createObjectURL(file) : undefined };
    uploads.value = [...uploads.value, entry];
    return uploads.value[uploads.value.length - 1];
  });
  const paths = await Promise.all(batch.map(uploadOne));
  const done = paths.filter((path): path is string => !!path);
  if (done.length) { terminal?.paste(pasteText(done)); terminal?.focus(); }
  for (const [index, entry] of batch.entries()) if (paths[index]) dropUpload(entry.id);
}
async function retryUpload(entry: Upload) {
  const path = await uploadOne(entry);
  if (path) { terminal?.paste(pasteText([path])); terminal?.focus(); dropUpload(entry.id); }
}
/** A paste carrying files is theirs; plain text is left to the terminal untouched. */
function onPaste(event: ClipboardEvent) {
  const files = filesFrom(event.clipboardData);
  if (!files.length) return;
  event.preventDefault(); event.stopPropagation();
  void sendFiles(files);
}
const carriesFiles = (event: DragEvent) => [...(event.dataTransfer?.types ?? [])].includes("Files");
function onDragOver(event: DragEvent) { if (!carriesFiles(event)) return; event.preventDefault(); if (event.dataTransfer) event.dataTransfer.dropEffect = "copy"; dragging.value = true; }
function onDragLeave(event: DragEvent) { if (!(event.currentTarget as Element).contains(event.relatedTarget as Node | null)) dragging.value = false; }
function onDrop(event: DragEvent) {
  dragging.value = false;
  const files = filesFrom(event.dataTransfer);
  if (!files.length) return;
  event.preventDefault(); event.stopPropagation();
  void sendFiles(files);
}
function pickFiles() { moreKeys.value = false; fileInput.value?.click(); }
function filesPicked(event: Event) {
  const input = event.target as HTMLInputElement;
  const files = [...(input.files ?? [])];
  input.value = "";
  void sendFiles(files);
}
/** The phone paste field takes images too: a long-press paste of a photo lands here. */
function onFieldPaste(event: ClipboardEvent) {
  const files = filesFrom(event.clipboardData);
  if (!files.length) return;
  event.preventDefault();
  pasteDraft.value = undefined;
  void sendFiles(files);
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

/**
 * Scroll with a finger.
 *
 * xterm.js 6 scrolls for a wheel and for nothing else -- its viewport no
 * longer follows touch -- so dragging the terminal on a phone moved nothing,
 * and a conversation the client had pushed into scrollback could not be read
 * again. A vertical drag is turned into what a wheel would have done: the
 * scrollback moves in the ordinary buffer, and a program that owns the screen
 * (the alternate buffer, or one that asked for mouse reports) is handed wheel
 * steps to scroll itself. Sideways drags are left to the pane swipe.
 */
let touchScroll: { x: number; y: number; axis?: "x" | "y"; carry: number } | undefined;
function touchScrollStart(event: TouchEvent) {
  const touch = event.touches[0];
  touchScroll = event.touches.length === 1 && touch ? { x: touch.clientX, y: touch.clientY, carry: 0 } : undefined;
}
function touchScrollMove(event: TouchEvent) {
  const current = touchScroll, touch = event.touches[0];
  if (!current || !touch || event.touches.length !== 1 || !terminal || !host.value) { touchScroll = undefined; return; }
  if (!current.axis) {
    current.axis = swipeAxis(touch.clientX - current.x, touch.clientY - current.y);
    if (current.axis === "x") touchScroll = undefined;
    if (current.axis !== "y") return;
  }
  event.preventDefault();
  const screen = host.value.querySelector<HTMLElement>(".xterm-screen");
  const rowHeight = (screen?.getBoundingClientRect().height ?? 0) / terminal.rows;
  if (!screen || !(rowHeight > 0)) return;
  // A finger moving up brings later lines into view, as a page does.
  current.carry += current.y - touch.clientY; current.y = touch.clientY;
  const lines = Math.trunc(current.carry / rowHeight);
  if (!lines) return;
  current.carry -= lines * rowHeight;
  if (terminal.buffer.active.type === "normal" && terminal.modes.mouseTrackingMode === "none") { terminal.scrollLines(lines); return; }
  // One wheel notch per line, so xterm.js reports or translates each exactly as it would a real one.
  for (let step = 0; step < Math.abs(lines); step++) {
    screen.dispatchEvent(new WheelEvent("wheel", { deltaY: Math.sign(lines), deltaMode: WheelEvent.DOM_DELTA_LINE, clientX: touch.clientX, clientY: touch.clientY, bubbles: true, cancelable: true }));
  }
}

function streamUrl(sessionId: string) {
  const url = new URL(`/api/sessions/${encodeURIComponent(sessionId)}/pty/ws`, window.location.href);
  url.protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  if (terminal) { url.searchParams.set("cols", String(terminal.cols)); url.searchParams.set("rows", String(terminal.rows)); }
  url.searchParams.set("framed", "true");
  if (resume?.runtime) { url.searchParams.set("runtime", resume.runtime); url.searchParams.set("after", String(resume.seq)); }
  return url.toString();
}
function connect() {
  if (disposed || !props.sessionId) return;
  try {
    initializeTerminal();
    const sessionId = props.sessionId;
    // Asked again on every automatic retry, so each one resumes from what the
    // screen shows by then.
    stream.connect(() => streamUrl(sessionId)); resize();
  } catch { stream.fail(TERMINAL_VIEW_ERROR); cleanupTerminal(); }
}
defineExpose({ reconnect: connect });

function terminalTheme() {
  return dark.value ? { background: "#17232d", foreground: "#dde6eb", cursor: "#63dac8", selectionBackground: "#2b4a46" } : {
    background: "#ffffff", foreground: "#243343", cursor: "#0a8278", selectionBackground: "#ccebe6",
    black: "#243343", brightBlack: "#657380", red: "#b4374c", brightRed: "#c73f55", green: "#187953", brightGreen: "#098462",
    yellow: "#916100", brightYellow: "#9b6800", blue: "#3467af", brightBlue: "#3375c5", magenta: "#7856aa", brightMagenta: "#8965bc",
    cyan: "#087e80", brightCyan: "#058587", white: "#607080", brightWhite: "#384658",
  };
}
watch(dark, () => { if (terminal) terminal.options.theme = terminalTheme(); });

function initializeTerminal() {
  if (terminal) return;
  if (!host.value) throw new Error("Terminal host is unavailable");
  terminal = new Terminal({
    cursorBlink: true, fontSize: 12, lineHeight: 1.22, scrollback: 5000,
    // A Nerd Font the viewer already has wins; the bundled symbols fill in the
    // prompt icons for everyone else, phones included.
    fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", "Symbols Nerd Font Mono", "AgentDock Symbols", monospace',
    theme: terminalTheme(),
  });
  fit = new FitAddon(); terminal.loadAddon(fit); terminal.open(host.value);
  subscriptions.push(terminal.onData(data => {
    if (shiftArmed.value) { shiftArmed.value = false; data = shiftSequence(data); }
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
    // A view that never got a terminal starts over; any other is woken, which
    // retries a dropped socket, replaces a stale one and probes the rest.
    if (!terminal && state.value === "error") connect(); else stream.wake(stale);
  });
});
watch(() => props.sessionId, async (_next, _previous, onCleanup) => {
  let current = true; onCleanup(() => { current = false; });
  stream.disconnect(); resume = undefined; terminal?.reset(); await nextTick(); if (current) connect();
});
onUnmounted(() => { for (const entry of uploads.value) { entry.abort?.(); if (entry.preview) URL.revokeObjectURL(entry.preview); } stopPageReturn(); disposed = true; stream.dispose(); cleanupTerminal(); emit("status", false); });
</script>

<template>
  <div :class="['native-terminal', { dark: dark, dragging }]" @paste.capture="onPaste" @dragover="onDragOver" @dragleave="onDragLeave" @drop="onDrop">
    <div class="terminal-connection"><span :class="['state-dot', state]" /><span>{{ t(state === 'connected' ? 'Live · native client' : state) }}</span><button v-if="state === 'disconnected' || state === 'error'" class="text-button" :aria-label="t('Reconnect session stream')" @click="connect"><Icon name="refresh" :size="13" />{{ t('Reconnect') }}</button><button v-if="state === 'connected'" type="button" class="text-button terminal-attach" :title="t('Add files or images: paste, drop, or pick them; their paths are typed into the terminal')" @click="pickFiles"><Icon name="plus" :size="13" />{{ t('Add file') }}</button></div>
    <input ref="fileInput" class="sr-only" type="file" multiple tabindex="-1" aria-hidden="true" @change="filesPicked" />
    <div v-if="dragging" class="terminal-drop" aria-hidden="true"><span>{{ t('Drop to add the files and type their paths') }}</span></div>
    <div v-if="uploads.length" class="terminal-uploads" role="status" aria-live="polite">
      <div v-for="entry in uploads" :key="entry.id" :class="['terminal-upload', { failed: entry.failed }]">
        <img v-if="entry.preview" :src="entry.preview" alt="" /><Icon v-else name="file" :size="16" />
        <span class="terminal-upload-text"><strong :title="entry.name">{{ entry.name }}</strong><small>{{ entry.failed ?? t('Uploading {percent}%', { percent: Math.round(entry.progress * 100) }) }}</small></span>
        <button v-if="entry.failed && entry.retryable" type="button" class="text-button" @click="retryUpload(entry)">{{ t('Retry') }}</button>
        <button type="button" class="icon-button" :aria-label="t(entry.failed ? 'Dismiss' : 'Cancel upload')" @click="dropUpload(entry.id)"><Icon name="close" :size="13" /></button>
      </div>
    </div>
    <div v-if="notice" :class="state === 'error' ? 'inline-error' : 'inline-notice'" :role="state === 'error' ? 'alert' : 'status'">{{ nativeNotice ? notice : t(notice) }}</div>
    <div ref="host" class="terminal-host" :aria-label="t('Native session {id}', { id: sessionId })" @pointerdown="gestureStart" @pointerup="gestureEnd" @pointercancel="gesture = undefined" @touchstart="touchScrollStart" @touchmove="touchScrollMove" />
    <div v-if="touchDevice" class="terminal-keys" role="group" :aria-label="t('Terminal keys')">
      <div v-if="moreKeys" class="terminal-keys-row">
        <button type="button" :aria-label="t('Paste')" :title="t('Paste')" @click="moreKeys = false; pressPaste()"><Icon name="clipboard" :size="16" /></button>
        <button type="button" class="key-text" :aria-label="t('Photo or file')" :title="t('Photo or file')" @click="pickFiles">{{ t('Photo / file') }}</button>
        <button type="button" :aria-label="t('Keyboard')" :title="t('Keyboard')" @click="moreKeys = false; showKeyboard()"><Icon name="edit" :size="16" /></button>
        <button type="button" :class="{ armed: tapSelect }" :aria-pressed="tapSelect" :aria-label="t('Tap a row to select it')" :title="t('Tap a row to select it')" @pointerdown="keepFocus" @click="tapSelect = !tapSelect"><Icon name="locate" :size="16" /></button>
      </div>
      <div class="terminal-keys-row">
        <button type="button" class="key-text" :aria-label="t('Escape')" :title="t('Escape')" @pointerdown="keepFocus" @click="pressEscape">Esc</button>
        <button type="button" class="key-text" :class="{ armed: ctrlArmed }" :aria-pressed="ctrlArmed" :aria-label="t('Ctrl')" :title="t('Ctrl')" @pointerdown="keepFocus" @click="ctrlArmed = !ctrlArmed">Ctrl</button>
        <button type="button" class="key-text" :class="{ armed: shiftArmed }" :aria-pressed="shiftArmed" :aria-label="t('Shift')" :title="t('Shift')" @pointerdown="keepFocus" @click="shiftArmed = !shiftArmed">⇧</button>
        <button type="button" class="key-text" :aria-label="t('Tab')" :title="t('Tab')" @pointerdown="keepFocus" @click="pressTab">Tab</button>
        <button type="button" class="key-text" :aria-label="t('Slash · command menu')" :title="t('Slash · command menu')" @pointerdown="keepFocus" @click="pressSlash">/</button>
        <span class="terminal-keys-sep" aria-hidden="true" />
        <button type="button" :aria-label="t('Arrow left')" :title="t('Arrow left')" @pointerdown="keepFocus" @click="pressArrow('left')"><Icon name="chevron" :size="16" class="key-left" /></button>
        <button type="button" :aria-label="t('Arrow up')" :title="t('Arrow up')" @pointerdown="keepFocus" @click="pressArrow('up')"><Icon name="chevron" :size="16" class="key-up" /></button>
        <button type="button" :aria-label="t('Arrow down')" :title="t('Arrow down')" @pointerdown="keepFocus" @click="pressArrow('down')"><Icon name="chevron" :size="16" class="key-down" /></button>
        <button type="button" :aria-label="t('Arrow right')" :title="t('Arrow right')" @pointerdown="keepFocus" @click="pressArrow('right')"><Icon name="chevron" :size="16" /></button>
        <button type="button" :aria-label="t('Enter')" :title="t('Enter')" @pointerdown="keepFocus" @click="pressEnter"><Icon name="check" :size="16" /></button>
        <span class="terminal-keys-sep" aria-hidden="true" />
        <button type="button" :class="{ armed: moreKeys || tapSelect }" :aria-expanded="moreKeys" :aria-label="t('More keys')" :title="t('More keys')" @pointerdown="keepFocus" @click="moreKeys = !moreKeys"><Icon name="more" :size="16" /></button>
      </div>
    </div>
    <form v-if="pasteDraft !== undefined" class="terminal-paste" @submit.prevent="sendPaste" @keydown.esc.prevent="cancelPaste">
      <textarea ref="pasteField" v-model="pasteDraft" :aria-label="t('Text to paste')" @paste="onFieldPaste" :placeholder="t('Long-press here to paste text, a photo or a file, then send it to the terminal')" />
      <div><button type="button" class="secondary-button" @click="cancelPaste">{{ t('Cancel') }}</button><button type="submit" class="primary-button" :disabled="!pasteDraft">{{ t('Send') }}</button></div>
    </form>
  </div>
</template>
