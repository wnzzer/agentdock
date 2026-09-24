<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue';
import ModalDialog from './ModalDialog.vue';
import { errorMessage, request } from './api';
import { MAX_HIGHLIGHT_BYTES, escapeHtml, languageFor, loadHighlighter } from './highlighting';
import { useI18n } from '../i18n';

/**
 * A file an agent mentioned that lives outside every workspace: shown
 * read-only, at the line it named. Editing belongs to a workspace; the way
 * there is adding the folder as one.
 */
const props = defineProps<{ path: string; line?: number }>();
const emit = defineEmits<{ close: []; addWorkspace: [folder: string] }>();
const { t } = useI18n();
const content = ref<string>(), error = ref(''), copied = ref(false), html = ref<string[]>();
const body = ref<HTMLElement>();
const name = computed(() => props.path.split('/').pop() || props.path);
const folder = computed(() => props.path.slice(0, props.path.lastIndexOf('/')) || '/');
const lines = computed(() => (content.value ?? '').split('\n'));

onMounted(async () => {
  try {
    const file = await request<{ content: string }>(`/host/file?${new URLSearchParams({ path: props.path })}`);
    content.value = file.content;
    const language = languageFor(props.path);
    if (language && file.content.length <= MAX_HIGHLIGHT_BYTES) {
      const highlight = await loadHighlighter(language);
      // Highlighted per line so the numbered rows and the target line stay aligned.
      if (highlight) html.value = file.content.split('\n').map(line => highlight(line, language) || escapeHtml(line));
    }
    await nextTick();
    if (props.line) body.value?.querySelector(`[data-line="${props.line}"]`)?.scrollIntoView({ block: 'center' });
  } catch (cause) { error.value = errorMessage(cause); }
});
async function copyPath() {
  try { await navigator.clipboard.writeText(props.path); copied.value = true; setTimeout(() => { copied.value = false; }, 1500); } catch { /* Clipboard refused; the path is on screen. */ }
}
</script>

<template>
  <ModalDialog :title="name" wide @close="emit('close')">
    <div class="host-file">
      <header>
        <code :title="path">{{ path }}</code>
        <span class="host-file-badge">{{ t('Read-only · outside every workspace') }}</span>
        <button type="button" class="small-button" @click="copyPath">{{ t(copied ? 'Copied' : 'Copy path') }}</button>
        <button type="button" class="small-button" @click="emit('addWorkspace', folder)">{{ t('Add folder as workspace') }}</button>
      </header>
      <p v-if="error" class="account-error" role="alert">{{ error }}</p>
      <p v-else-if="content === undefined" class="host-file-quiet">{{ t('Loading…') }}</p>
      <div v-else ref="body" class="host-file-body">
        <div v-for="(text, index) in lines" :key="index" :data-line="index + 1" :class="['host-file-line', { target: index + 1 === line }]"><span>{{ index + 1 }}</span><code v-if="html" v-html="html[index]" /><code v-else>{{ text }}</code></div>
      </div>
    </div>
  </ModalDialog>
</template>

<style scoped>
.host-file{display:flex;flex-direction:column;gap:10px;min-height:0;height:min(620px,calc(100vh - 200px))}
.host-file header{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.host-file header code{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:11.5px;color:var(--ink-soft)}
.host-file-badge{flex-shrink:0;font-size:10.5px;padding:2px 8px;border-radius:6px;background:var(--fill);color:var(--muted)}
.host-file-quiet{font-size:12px;color:var(--muted)}
.host-file-body{flex:1;min-height:0;overflow:auto;border:1px solid var(--border);border-radius:10px;background:var(--surface);padding:8px 0;font:12px/1.65 ui-monospace,SFMono-Regular,Menlo,monospace}
.host-file-line{display:flex;min-width:max-content}
.host-file-line>span{flex:none;width:48px;padding-right:12px;text-align:right;color:var(--muted);user-select:none}
.host-file-line>code{white-space:pre;padding-right:16px;color:var(--ink);font:inherit}
.host-file-line.target{background:#fff6d6}
.host-file-line.target>span{color:var(--ink)}
</style>
