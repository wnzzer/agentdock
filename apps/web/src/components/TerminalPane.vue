<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import Icon from "../features/Icon.vue";
import { useI18n } from "../i18n";
import { createSessionStream, TERMINAL_VIEW_ERROR } from "../features/session-stream";
import type { SessionStreamState } from "../features/session-stream";
const { t } = useI18n();

const props = withDefaults(defineProps<{ sessionId: string; dark?: boolean }>(), { dark: false });
const emit = defineEmits<{ exit: []; status: [connected: boolean] }>();
const host = ref<HTMLDivElement>();
const state = ref<SessionStreamState>("connecting");
const notice = ref("");
const nativeNotice = ref(false);
let terminal: Terminal | undefined;
let fit: FitAddon | undefined;
let observer: ResizeObserver | undefined;
let resizeFrame = 0;
let disposed = false;
const subscriptions: Array<{ dispose: () => void }> = [];
const stream = createSessionStream({
  createSocket: url => new WebSocket(url),
  onState(next, message) { state.value = next; notice.value = message; nativeNotice.value = false; emit("status", next === "connected"); },
  onOpen: resize,
  onMessage(data) {
    if (data instanceof ArrayBuffer) { terminal?.write(new Uint8Array(data)); return; }
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
    fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", monospace',
    theme: props.dark ? { background: "#17232d", foreground: "#dde6eb", cursor: "#63dac8" } : {
      background: "#ffffff", foreground: "#243343", cursor: "#0a8278", selectionBackground: "#ccebe6",
      black: "#243343", brightBlack: "#657380", red: "#b4374c", brightRed: "#c73f55", green: "#187953", brightGreen: "#098462",
      yellow: "#916100", brightYellow: "#9b6800", blue: "#3467af", brightBlue: "#3375c5", magenta: "#7856aa", brightMagenta: "#8965bc",
      cyan: "#087e80", brightCyan: "#058587", white: "#607080", brightWhite: "#384658",
    },
  });
  fit = new FitAddon(); terminal.loadAddon(fit); terminal.open(host.value);
  subscriptions.push(terminal.onData(data => { stream.send(new TextEncoder().encode(data)); }));
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
onMounted(connect);
watch(() => props.sessionId, async (_next, _previous, onCleanup) => {
  let current = true; onCleanup(() => { current = false; });
  stream.disconnect(); await nextTick(); if (current) connect();
});
onUnmounted(() => { disposed = true; stream.dispose(); cleanupTerminal(); emit("status", false); });
</script>

<template>
  <div :class="['native-terminal', { dark }]">
    <div class="terminal-connection"><span :class="['state-dot', state]" /><span>{{ t(state === 'connected' ? 'Live · native client' : state) }}</span><button v-if="state === 'disconnected' || state === 'error'" class="text-button" :aria-label="t('Reconnect session stream')" @click="connect"><Icon name="refresh" :size="13" />{{ t('Reconnect') }}</button></div>
    <div v-if="notice" :class="state === 'error' ? 'inline-error' : 'inline-notice'" :role="state === 'error' ? 'alert' : 'status'">{{ nativeNotice ? notice : t(notice) }}</div>
    <div ref="host" class="terminal-host" :aria-label="t('Native session {id}', { id: sessionId })" />
  </div>
</template>
