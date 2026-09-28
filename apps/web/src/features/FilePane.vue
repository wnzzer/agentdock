<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import type { TextFile } from "@agentdock/protocol";
import { ApiError, assetUrl, errorMessage, json, request, workspacePath } from "./api";
import { fileDraft } from "./file-drafts";
import { lineRange, pendingJump, takeJump } from "./file-jumps";
import { onFilesChanged, touchesFile } from "./file-watch";
import { escapeHtml, languageFor, loadHighlighter, MAX_HIGHLIGHT_BYTES, wrapsProse } from "./highlighting";
import { MarkdownContent } from "./MarkdownContent";
import { framedAnchors, lineAtOffset, lineForPreviewTop, offsetAtLine, previewTopForLine, type Anchor } from "./markdown-sync";
import { continueList, indentLines, type TextEdit } from "./markdown-editing";
import Icon from "./Icon.vue";
import { useI18n } from "../i18n";
const { t } = useI18n();
const props = defineProps<{ workspaceId: string; path?: string }>();
const emit = defineEmits<{ saved: []; browse: []; reveal: [path: string] }>();
const loading = ref(false), saving = ref(false), error = ref(""), success = ref("");
const conflict = ref(false), confirmReload = ref(false), mediaFailed = ref(false);
const draft = computed(() => fileDraft(props.workspaceId, props.path ?? ""));
const dirty = computed(() => draft.value.loaded && draft.value.content !== draft.value.original);
const extension = computed(() => props.path?.split(".").pop()?.toLowerCase() ?? "");
const kind = computed(() => /^(png|jpg|jpeg|gif|webp|svg|bmp|ico|avif)$/.test(extension.value) ? "image" : /^(mp4|webm|mov|m4v|ogv)$/.test(extension.value) ? "video" : /^(mp3|wav|ogg|m4a|flac)$/.test(extension.value) ? "audio" : extension.value === "pdf" ? "pdf" : "text");
const mediaVersion = ref(0);
const url = computed(() => props.path ? assetUrl(props.workspaceId, props.path) + (mediaVersion.value ? `&v=${mediaVersion.value}` : "") : "");
/**
 * Syntax highlighting, drawn behind the editor rather than replacing it.
 *
 * A textarea cannot hold markup, so the highlighted copy sits underneath and
 * the textarea on top is transparent apart from its caret. Both must lay text
 * out identically — same font, padding, wrapping and tab size — or the two
 * drift apart as you scroll.
 */
const language = computed(() => props.path ? languageFor(props.path) : undefined);
const oversize = computed(() => draft.value.content.length > MAX_HIGHLIGHT_BYTES);
const highlighter = ref<((source: string, language: string) => string) | undefined>();
watch([language, oversize], ([lang, big]) => {
  if (!lang || big || highlighter.value) return;
  void loadHighlighter(lang).then(loaded => { highlighter.value = loaded; }).catch(() => {});
}, { immediate: true });
const highlighted = computed(() => {
  const source = draft.value.content;
  if (!language.value || oversize.value || !highlighter.value) return undefined;
  // The trailing newline keeps the last line's height when the file ends in one,
  // so the final row of the overlay lines up with the textarea's.
  return highlighter.value(source, language.value) + (source.endsWith("\n") ? "\n" : "");
});
/**
 * While an input method is composing, the editor shows its own text.
 *
 * What you normally read is the highlighted copy underneath; the textarea over
 * it is transparent apart from its caret. But a composing input method draws
 * its candidate text in the textarea, and the value behind the overlay does not
 * receive it until the composition is committed -- so pinyin was being drawn in
 * transparent ink over a layer that did not yet know about it, and typing
 * Chinese looked like typing nothing at all.
 *
 * For the length of the composition the textarea inks itself and covers the
 * overlay. Highlighting comes back the moment the text is committed, which is
 * also the moment there is anything to highlight.
 */
const composing = ref(false);
const highlightLayer = ref<HTMLElement>();
const editor = ref<HTMLTextAreaElement>();
/**
 * The overlay ends where the textarea's text does.
 *
 * A vertical scrollbar takes its width from the textarea and not from the layer
 * behind it. Eleven pixels sounds like nothing, but wrapped text re-breaks at a
 * different place in a column that narrow, and the highlighted copy drifted a
 * whole line further out with every paragraph. Measured rather than assumed:
 * how much a scrollbar takes, or whether it takes anything at all, is the
 * platform's business.
 */
const gutter = ref(0);
function measureGutter() {
  const element = editor.value;
  gutter.value = element ? Math.max(0, element.offsetWidth - element.clientWidth) : 0;
}
let watcher: ResizeObserver | undefined;
watch(editor, element => {
  watcher?.disconnect(); watcher = undefined;
  if (!element) return;
  // A scrollbar appears when the content grows, which changes the content box
  // without changing the border box -- so the content box is what is watched.
  watcher = new ResizeObserver(measureGutter);
  watcher.observe(element);
  measureGutter();
});
/**
 * The overlay follows the textarea by moving its content, not by scrolling.
 *
 * Scrolling it clamps to its own maximum, and the two maxima differ: the
 * textarea's viewport is shorter by the height of its own scrollbar. That put
 * the last line 11px out of register at the bottom of a file. A transform has
 * no such limit, so the layers stay locked together everywhere.
 */
function syncScroll(event: Event) {
  const source = event.target as HTMLTextAreaElement;
  const content = highlightLayer.value?.firstElementChild as HTMLElement | undefined;
  if (content) content.style.transform = `translate(${-source.scrollLeft}px, ${-source.scrollTop}px)`;
}

/**
 * Prose wraps; code does not.
 *
 * A column that has to line up is the reason a code editor scrolls sideways
 * instead of wrapping, and markdown has no such column. A paragraph of Chinese
 * is one long line by nature, so left unwrapped it ran off the right edge and
 * was written through a horizontal scrollbar.
 *
 * Both layers wrap together or not at all: they share every metric so that the
 * highlighted copy stays registered with the text on top of it.
 */
const wrapped = computed(() => wrapsProse(language.value));
/**
 * Markdown is read and written side by side: the source on the left, the
 * rendered result on the right, scrolled together. A pane too narrow for two
 * columns -- a phone, a split canvas -- switches between them instead, and
 * keeps its place when it does. The chosen view is remembered per browser.
 */
const isMarkdown = computed(() => language.value === "markdown");
type MarkdownMode = "preview" | "split" | "source";
const MODE_KEY = "agentdock.markdown-mode.v1";
function readMode(): MarkdownMode {
  try { const value = localStorage.getItem(MODE_KEY); return value === "preview" || value === "source" || value === "split" ? value : "split"; }
  catch { return "split"; }
}
const mode = ref<MarkdownMode>(readMode());
const pane = ref<HTMLElement>(), preview = ref<HTMLElement>(), mirror = ref<HTMLElement>();
const paneWidth = ref(0);
const canSplit = computed(() => paneWidth.value >= 720);
/** What is on screen. Two columns that do not fit become the rendered view. */
const view = computed<MarkdownMode>(() => !isMarkdown.value ? "source" : mode.value === "split" && !canSplit.value ? "preview" : mode.value);
const showEditor = computed(() => view.value !== "preview");
const showPreview = computed(() => isMarkdown.value && view.value !== "source");
const modeChoices = computed<{ mode: MarkdownMode; label: string }[]>(() => canSplit.value
  ? [{ mode: "preview", label: "Preview" }, { mode: "split", label: "Split" }, { mode: "source", label: "Source" }]
  : [{ mode: "preview", label: "Preview" }, { mode: "source", label: "Edit" }]);
let paneWatcher: ResizeObserver | undefined;
watch(pane, element => {
  paneWatcher?.disconnect(); paneWatcher = undefined;
  if (!element) return;
  paneWatcher = new ResizeObserver(([entry]) => { if (entry) paneWidth.value = entry.contentRect.width; });
  paneWatcher.observe(element);
});

/*
 * Keeping the two sides on the same passage (see markdown-sync.ts). The
 * editor is measured through a hidden copy of its text, one element per
 * line, laid out with the same metrics -- so a long wrapped paragraph counts
 * as the several rows it really takes. Measured only when asked, and again
 * only after the text or the width has changed.
 */
let measured = { content: "", width: -1 }, offsets: number[] = [];
function measure(): number[] {
  const box = mirror.value, input = editor.value;
  if (!box || !input) return offsets;
  const content = draft.value.content, width = input.clientWidth;
  if (offsets.length && content === measured.content && width === measured.width) return offsets;
  box.innerHTML = content.split("\n").map(line => `<div>${escapeHtml(line) || "\u200b"}</div>`).join("");
  offsets = Array.from(box.children, child => (child as HTMLElement).offsetTop);
  measured = { content, width };
  return offsets;
}
function anchors(): Anchor[] {
  const box = preview.value;
  if (!box) return [];
  const origin = box.getBoundingClientRect().top - box.scrollTop;
  const found = Array.from(box.querySelectorAll<HTMLElement>("[data-line]"), element => ({ line: Number(element.dataset.line), top: element.getBoundingClientRect().top - origin }));
  return framedAnchors(found, draft.value.content.split("\n").length, box.scrollHeight);
}
/** Which side is being scrolled by hand; the other only follows. */
let driver: "editor" | "preview" | "program" | undefined, driverTimer: ReturnType<typeof setTimeout> | undefined;
function drive(side: "editor" | "preview" | "program") {
  if (driver && driver !== side) return false;
  driver = side; clearTimeout(driverTimer);
  driverTimer = setTimeout(() => { driver = undefined; }, 150);
  return true;
}
function followEditor() {
  const input = editor.value, box = preview.value;
  if (input && box) box.scrollTop = previewTopForLine(anchors(), lineAtOffset(measure(), input.scrollTop));
}
function editorScrolled(event: Event) {
  syncScroll(event);
  if (view.value === "split" && drive("editor")) followEditor();
}
function previewScrolled() {
  const input = editor.value, box = preview.value;
  if (view.value !== "split" || !input || !box || !drive("preview")) return;
  input.scrollTop = offsetAtLine(measure(), lineForPreviewTop(anchors(), box.scrollTop));
  syncScroll({ target: input } as unknown as Event);
}
/** The source line at the top of whichever side is showing. */
function currentLine(): number | undefined {
  if (showEditor.value && editor.value) return lineAtOffset(measure(), editor.value.scrollTop);
  if (showPreview.value && preview.value) return lineForPreviewTop(anchors(), preview.value.scrollTop);
  return undefined;
}
function showLine(line: number, from = 0) {
  drive("program");
  const input = editor.value, box = preview.value;
  if (showEditor.value && input) { input.scrollTop = Math.max(0, offsetAtLine(measure(), line) - from); syncScroll({ target: input } as unknown as Event); }
  if (showPreview.value && box) box.scrollTop = Math.max(0, previewTopForLine(anchors(), input && showEditor.value ? lineAtOffset(measure(), input.scrollTop) : line) - (input && showEditor.value ? 0 : from));
}
function rememberMode(next: MarkdownMode) {
  mode.value = next;
  try { localStorage.setItem(MODE_KEY, next); } catch { /* Remembered for this page only. */ }
}
/** Switching views keeps the passage you were reading on screen. */
async function setMode(next: MarkdownMode) {
  const line = currentLine();
  rememberMode(next);
  await nextTick();
  if (line !== undefined) showLine(line);
}
/** Double-click a passage in the preview to write there. */
async function editAt(event: MouseEvent) {
  const block = (event.target as HTMLElement | null)?.closest<HTMLElement>("[data-line]");
  if (!block) return;
  const line = Number(block.dataset.line);
  if (!showEditor.value) { rememberMode(canSplit.value ? "split" : "source"); await nextTick(); }
  const input = editor.value;
  if (!input) return;
  const [start] = lineRange(draft.value.content, line + 1);
  input.focus({ preventScroll: true });
  input.setSelectionRange(start, start);
  showLine(line, input.clientHeight / 3);
}
// Typing in the source moves the preview along with it.
watch(() => draft.value.content, () => {
  if (view.value === "split" && document.activeElement === editor.value) requestAnimationFrame(() => { if (drive("editor")) followEditor(); });
});

/** Enter continues a list, Tab indents it (markdown-editing.ts). Kept undoable. */
function applyEdit(input: HTMLTextAreaElement, edit: TextEdit) {
  input.setSelectionRange(edit.from, edit.to);
  // execCommand is what puts the change on the textarea's own undo stack.
  const done = edit.insert ? document.execCommand("insertText", false, edit.insert) : edit.from === edit.to || document.execCommand("delete");
  if (!done) { input.setRangeText(edit.insert, edit.from, edit.to, "end"); input.dispatchEvent(new Event("input", { bubbles: true })); }
  input.setSelectionRange(edit.selectionStart, edit.selectionEnd);
}
function editorKey(event: KeyboardEvent) {
  if (!isMarkdown.value || composing.value || event.isComposing || event.keyCode === 229 || event.altKey || event.metaKey || event.ctrlKey) return;
  const input = event.target as HTMLTextAreaElement;
  if (event.key === "Enter" && !event.shiftKey && input.selectionStart === input.selectionEnd) {
    const edit = continueList(input.value, input.selectionStart);
    if (edit) { event.preventDefault(); applyEdit(input, edit); }
  } else if (event.key === "Tab") {
    event.preventDefault();
    const edit = indentLines(input.value, input.selectionStart, input.selectionEnd, event.shiftKey);
    if (edit) applyEdit(input, edit);
  }
}
/**
 * Show a line another pane asked for -- a file reference clicked in chat. The
 * line is selected, which is also its highlight, and scrolled to a third of
 * the way down. Rendered Markdown has no lines, so its source is shown.
 */
watch(() => [draft.value.loaded, editor.value, pendingJump(props.workspaceId, props.path ?? '')?.at] as const, async ([loaded]) => {
  if (!loaded || !props.path || !pendingJump(props.workspaceId, props.path)) return;
  if (!showEditor.value) { mode.value = canSplit.value ? "split" : "source"; await nextTick(); }
  const input = editor.value; if (!input) return;
  const line = takeJump(props.workspaceId, props.path); if (!line) return;
  const [start, end] = lineRange(draft.value.content, line);
  input.focus({ preventScroll: true });
  input.setSelectionRange(start, end);
  const height = parseFloat(getComputedStyle(input).lineHeight) || 20;
  input.scrollTop = Math.max(0, (line - 1) * height - input.clientHeight / 3);
  // The highlighted layer beneath follows scroll events only; a scroll set
  // from code has to move it too, now and once highlighting has drawn.
  syncScroll({ target: input } as unknown as Event);
  requestAnimationFrame(() => syncScroll({ target: input } as unknown as Event));
}, { immediate: true });

let revision = 0;
async function read() {
  if (!props.path || kind.value !== "text") return;
  const requestId = ++revision, target = draft.value;
  loading.value = true; error.value = ""; success.value = ""; confirmReload.value = false;
  try {
    const file = await request<TextFile>(`${workspacePath(props.workspaceId)}/file?path=${encodeURIComponent(props.path)}`);
    if (requestId !== revision) return;
    Object.assign(target, { content: file.content, original: file.content, version: file.version, loaded: true });
    conflict.value = false;
  } catch (cause) { if (requestId === revision) error.value = errorMessage(cause); }
  finally { if (requestId === revision) loading.value = false; }
}
async function save() {
  if (!props.path || !dirty.value || saving.value) return;
  const target = draft.value, sentContent = target.content;
  const own = revision, workspaceId = props.workspaceId, path = props.path;
  saving.value = true; error.value = ""; success.value = "";
  try {
    const file = await request<TextFile>(`${workspacePath(workspaceId)}/file?path=${encodeURIComponent(path)}`, json("PUT", { content: sentContent, expected_version: target.version }));
    target.original = sentContent; target.version = file.version;
    if (own === revision) { conflict.value = false; success.value = "Saved to host"; emit("saved"); }
  } catch (cause) { if (own === revision) { conflict.value = cause instanceof ApiError && cause.status === 409; error.value = conflict.value ? "This file changed on the host. Your draft is kept. Copy your draft, or reload the host version before saving." : errorMessage(cause); } }
  finally { if (own === revision) saving.value = false; }
}
function reload() { if (dirty.value) confirmReload.value = true; else void read(); }
/**
 * The host changed this file. An untouched copy follows it where you were
 * reading; a draft is never overwritten, only warned about before it is saved
 * over the new version.
 */
async function hostChanged() {
  if (!props.path || !draft.value.loaded || saving.value || loading.value) return;
  const own = revision, target = draft.value;
  try {
    const file = await request<TextFile>(`${workspacePath(props.workspaceId)}/file?path=${encodeURIComponent(props.path)}`);
    if (own !== revision || saving.value || file.version === target.version) return;
    if (target.content !== target.original) { conflict.value = true; error.value = "This file changed on the host. Your draft is kept. Copy your draft, or reload the host version before saving."; return; }
    const input = editor.value, at = input && { start: input.selectionStart, end: input.selectionEnd, top: input.scrollTop, left: input.scrollLeft };
    Object.assign(target, { content: file.content, original: file.content, version: file.version });
    if (!input || !at) return;
    await nextTick();
    input.setSelectionRange(at.start, at.end); input.scrollTop = at.top; input.scrollLeft = at.left;
    syncScroll({ target: input } as unknown as Event);
  } catch { /* Deleted or unreadable: an explicit reload says why. */ }
}
const stopWatching = onFilesChanged((workspaceId, paths) => {
  if (workspaceId !== props.workspaceId || !props.path || !touchesFile(paths, props.path)) return;
  if (kind.value === "text") void hostChanged();
  else { mediaVersion.value++; mediaFailed.value = false; }
});
watch([() => props.workspaceId, () => props.path], () => { revision++; saving.value = false; error.value = ""; success.value = ""; mediaFailed.value = false; loading.value = false; conflict.value = false; confirmReload.value = false; if (!draft.value.loaded) void read(); }, { immediate: true, flush: "sync" });
onBeforeUnmount(() => { revision++; watcher?.disconnect(); paneWatcher?.disconnect(); clearTimeout(driverTimer); stopWatching(); });
</script>
<template>
  <div v-if="!path" class="pane-empty"><span class="empty-icon"><Icon name="file" :size="28" /></span><h3>{{ t('Your files, right here') }}</h3><p>{{ t('Open a file from Explorer, or drag it into this pane.') }}</p><button class="secondary-button" @click="emit('browse')">{{ t('Browse files') }}</button></div>
  <div v-else ref="pane" class="file-pane" @keydown.meta.s.prevent="save" @keydown.ctrl.s.prevent="save">
    <div class="content-toolbar"><span class="truncate" :title="path">{{ path }}<b v-if="dirty" class="dirty-indicator" :title="t('Unsaved changes')">●</b></span><div class="toolbar-buttons"><button class="icon-button" :aria-label="t('Reveal in file tree')" :title="t('Reveal in file tree')" @click="emit('reveal', path)"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="6"/><path d="M12 2v4m0 12v4M2 12h4m12 0h4"/><circle cx="12" cy="12" r="1"/></svg></button><a class="icon-button" :href="url" download :aria-label="t('Download {path}', { path })"><Icon name="download" :size="15" /></a><div v-if="isMarkdown && kind === 'text'" class="md-modes" role="group" :aria-label="t('Markdown view')"><button v-for="choice in modeChoices" :key="choice.mode" type="button" :class="{ active: view === choice.mode }" :aria-pressed="view === choice.mode" @click="setMode(choice.mode)">{{ t(choice.label) }}</button></div><template v-if="kind === 'text'"><button class="icon-button" :aria-label="t('Reload file from host')" :disabled="loading || saving" @click="reload"><Icon name="refresh" :size="15" /></button><button class="small-button primary" :disabled="!dirty || saving || loading" @click="save"><Icon name="save" :size="13" />{{ t(saving ? 'Saving…' : dirty ? 'Save' : 'Saved') }}</button></template></div></div>
    <div v-if="error" class="inline-error" role="alert">{{ conflict ? t(error) : error }}</div>
    <div v-if="success" class="inline-success" role="status">{{ t(success) }}</div>
    <div v-if="confirmReload" class="confirmation-bar">{{ t('Reloading discards this unsaved draft.') }}<button class="small-button danger" @click="read">{{ t('Discard draft & reload') }}</button><button class="small-button" @click="confirmReload = false">{{ t('Keep draft') }}</button></div>
    <div v-if="loading" class="pane-empty"><p>{{ t('Reading file from host…') }}</p></div>
    <div v-else-if="kind === 'text' && draft.loaded" :class="['text-body', { 'is-split': view === 'split' }]">
      <div v-if="showEditor" class="code-surface" :class="{ 'is-wrapped': wrapped }">
        <pre v-if="highlighted !== undefined" ref="highlightLayer" class="code-highlight" :style="{ right: gutter + 'px' }" aria-hidden="true"><code v-html="highlighted" /></pre>
        <div v-if="isMarkdown" ref="mirror" class="code-mirror" :style="{ right: gutter + 'px' }" aria-hidden="true" />
        <textarea ref="editor" v-model="draft.content" :class="['code-editor', { 'is-overlaid': highlighted !== undefined, 'is-composing': composing }]" :aria-label="t('Edit {path}', { path })" spellcheck="false" autocapitalize="off" autocomplete="off" @input="success = ''" @scroll="editorScrolled" @keydown="editorKey" @compositionstart="composing = true" @compositionend="composing = false" />
      </div>
      <div v-if="showPreview" ref="preview" class="markdown-preview" :title="view === 'preview' ? t('Double-click a passage to edit it') : undefined" @scroll="previewScrolled" @dblclick="editAt"><MarkdownContent :text="draft.content" :image-url="(p: string) => assetUrl(workspaceId, p)" :base="path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : ''" source-lines /></div>
    </div>
    <div v-else-if="kind === 'image'" class="media-preview image-preview"><img v-if="!mediaFailed" :src="url" :alt="path" @error="mediaFailed = true" /><p v-else>{{ t('Unable to preview this image. It may be unavailable or unsupported.') }}</p></div>
    <div v-else-if="kind === 'video'" class="media-preview"><video :src="url" controls preload="metadata" @error="mediaFailed = true" /><p v-if="mediaFailed">{{ t('This browser cannot play this file. Use Download to open it locally.') }}</p></div>
    <div v-else-if="kind === 'audio'" class="media-preview"><audio :src="url" controls preload="metadata" @error="mediaFailed = true" /><p v-if="mediaFailed">{{ t('This browser cannot play this file.') }}</p></div>
    <iframe v-else-if="kind === 'pdf'" :src="url" class="pdf-preview" :title="t('PDF preview: {path}', { path })" />
    <div v-else class="pane-empty"><Icon name="file" :size="28" /><p>{{ t('Text preview is unavailable for this file.') }}</p><a class="secondary-button" :href="url" download>{{ t('Download file') }}</a></div>
    <div class="content-footer"><span>{{ kind !== 'text' ? t(`${kind} preview`) : isMarkdown && view === 'preview' ? t('Rendered markdown') : oversize ? t('Plain text · too large to highlight') : language ? language : t('Plain text editor') }}</span><span v-if="kind === 'text' && draft.loaded">{{ t('{count} lines · {status}', { count: draft.content.split('\n').length, status: t(dirty ? 'Unsaved draft' : 'In sync at last read') }) }}</span></div>
  </div>
</template>
