<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { TextFile } from "@agentdock/protocol";
import { ApiError, assetUrl, errorMessage, json, request, workspacePath } from "./api";
import { fileDraft } from "./file-drafts";
import { escapeHtml, languageFor, loadHighlighter, MAX_HIGHLIGHT_BYTES } from "./highlighting";
import { MarkdownContent } from "./MarkdownContent";
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
const url = computed(() => props.path ? assetUrl(props.workspaceId, props.path) : "");
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
const highlightLayer = ref<HTMLElement>();
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

/** Markdown is shown rendered by default, with the source one click away. */
const isMarkdown = computed(() => language.value === "markdown");
const showSource = ref(false);
watch(() => props.path, () => { showSource.value = false; });

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
watch([() => props.workspaceId, () => props.path], () => { revision++; saving.value = false; error.value = ""; success.value = ""; mediaFailed.value = false; loading.value = false; conflict.value = false; confirmReload.value = false; if (!draft.value.loaded) void read(); }, { immediate: true, flush: "sync" });
onBeforeUnmount(() => { revision++; });
</script>
<template>
  <div v-if="!path" class="pane-empty"><span class="empty-icon"><Icon name="file" :size="28" /></span><h3>{{ t('Your files, right here') }}</h3><p>{{ t('Open a file from Explorer, or drag it into this pane.') }}</p><button class="secondary-button" @click="emit('browse')">{{ t('Browse files') }}</button></div>
  <div v-else class="file-pane" @keydown.meta.s.prevent="save" @keydown.ctrl.s.prevent="save">
    <div class="content-toolbar"><span class="truncate" :title="path">{{ path }}<b v-if="dirty" class="dirty-indicator" :title="t('Unsaved changes')">●</b></span><div class="toolbar-buttons"><button class="icon-button" :aria-label="t('Reveal in file tree')" :title="t('Reveal in file tree')" @click="emit('reveal', path)"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="6"/><path d="M12 2v4m0 12v4M2 12h4m12 0h4"/><circle cx="12" cy="12" r="1"/></svg></button><a class="icon-button" :href="url" download :aria-label="t('Download {path}', { path })"><Icon name="download" :size="15" /></a><button v-if="isMarkdown" class="small-button" :aria-label="t(showSource ? 'Show rendered markdown' : 'Show markdown source')" @click="showSource = !showSource"><Icon :name="showSource ? 'file' : 'edit'" :size="13" />{{ t(showSource ? 'Preview' : 'Source') }}</button><template v-if="kind === 'text'"><button class="icon-button" :aria-label="t('Reload file from host')" :disabled="loading || saving" @click="reload"><Icon name="refresh" :size="15" /></button><button class="small-button primary" :disabled="!dirty || saving || loading" @click="save"><Icon name="save" :size="13" />{{ t(saving ? 'Saving…' : dirty ? 'Save' : 'Saved') }}</button></template></div></div>
    <div v-if="error" class="inline-error" role="alert">{{ conflict ? t(error) : error }}</div>
    <div v-if="success" class="inline-success" role="status">{{ t(success) }}</div>
    <div v-if="confirmReload" class="confirmation-bar">{{ t('Reloading discards this unsaved draft.') }}<button class="small-button danger" @click="read">{{ t('Discard draft & reload') }}</button><button class="small-button" @click="confirmReload = false">{{ t('Keep draft') }}</button></div>
    <div v-if="loading" class="pane-empty"><p>{{ t('Reading file from host…') }}</p></div>
    <div v-else-if="kind === 'text' && draft.loaded && isMarkdown && !showSource" class="markdown-preview"><MarkdownContent :text="draft.content" /></div>
    <div v-else-if="kind === 'text' && draft.loaded" class="code-surface">
      <pre v-if="highlighted !== undefined" ref="highlightLayer" class="code-highlight" aria-hidden="true"><code v-html="highlighted" /></pre>
      <textarea v-model="draft.content" :class="['code-editor', { 'is-overlaid': highlighted !== undefined }]" :aria-label="t('Edit {path}', { path })" spellcheck="false" autocapitalize="off" autocomplete="off" @input="success = ''" @scroll="syncScroll" />
    </div>
    <div v-else-if="kind === 'image'" class="media-preview image-preview"><img v-if="!mediaFailed" :src="url" :alt="path" @error="mediaFailed = true" /><p v-else>{{ t('Unable to preview this image. It may be unavailable or unsupported.') }}</p></div>
    <div v-else-if="kind === 'video'" class="media-preview"><video :src="url" controls preload="metadata" @error="mediaFailed = true" /><p v-if="mediaFailed">{{ t('This browser cannot play this file. Use Download to open it locally.') }}</p></div>
    <div v-else-if="kind === 'audio'" class="media-preview"><audio :src="url" controls preload="metadata" @error="mediaFailed = true" /><p v-if="mediaFailed">{{ t('This browser cannot play this file.') }}</p></div>
    <iframe v-else-if="kind === 'pdf'" :src="url" class="pdf-preview" :title="t('PDF preview: {path}', { path })" />
    <div v-else class="pane-empty"><Icon name="file" :size="28" /><p>{{ t('Text preview is unavailable for this file.') }}</p><a class="secondary-button" :href="url" download>{{ t('Download file') }}</a></div>
    <div class="content-footer"><span>{{ kind !== 'text' ? t(`${kind} preview`) : isMarkdown && !showSource ? t('Rendered markdown') : oversize ? t('Plain text · too large to highlight') : language ? language : t('Plain text editor') }}</span><span v-if="kind === 'text' && draft.loaded">{{ t('{count} lines · {status}', { count: draft.content.split('\n').length, status: t(dirty ? 'Unsaved draft' : 'In sync at last read') }) }}</span></div>
  </div>
</template>
