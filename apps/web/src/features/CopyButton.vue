<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue';
import { useI18n } from '../i18n';
import { copyText } from './clipboard';
import BottomSheet from './BottomSheet.vue';

const props = defineProps<{ text: string; label: string }>();
const { t } = useI18n();
const copied = ref(false), manual = ref(false), busy = ref(false);
const manualText = ref('');
let timer: ReturnType<typeof setTimeout> | undefined;
onBeforeUnmount(() => clearTimeout(timer));
async function copy() {
  busy.value = true;
  const text = props.text;
  try {
    await copyText(text);
    copied.value = true;
    clearTimeout(timer);
    timer = setTimeout(() => { copied.value = false; }, 1800);
  } catch { manualText.value = text; manual.value = true; }
  finally { busy.value = false; }
}
</script>

<template>
  <button type="button" class="copy-text-button" :disabled="busy || !text" :aria-label="t(label)" @click.stop="copy"><span role="status">{{ t(copied ? 'Copied' : label) }}</span></button>
  <BottomSheet v-if="manual" :title="t('Select text to copy')" @close="manual=false">
    <template #default="{ close }">
      <p class="copy-help">{{ t('Automatic copy is unavailable. Long-press the text below and choose Copy.') }}</p>
      <textarea class="copy-manual-text" :value="manualText" readonly :aria-label="t('Select text to copy')" @contextmenu.stop />
      <button type="button" class="copy-text-button" @click="close">{{ t('Close') }}</button>
    </template>
  </BottomSheet>
</template>

<style scoped>
.copy-text-button{display:inline-flex;align-items:center;justify-content:center;min-height:36px;min-width:64px;padding:6px 10px;border:1px solid var(--border);border-radius:8px;background:var(--surface);color:var(--ink-soft);font:inherit;font-size:12px;cursor:pointer;touch-action:manipulation;user-select:none}
.copy-text-button:focus-visible{outline:2px solid var(--focus);outline-offset:2px}
.copy-text-button:disabled{opacity:.5}
.copy-help{margin:8px 10px;font-size:14px;line-height:1.7}
.copy-manual-text{display:block;width:100%;height:35dvh;padding:12px;border:1px solid var(--border);border-radius:8px;background:var(--surface);color:var(--ink);font:16px/1.6 ui-monospace,monospace;resize:vertical;-webkit-user-select:text;user-select:text;-webkit-touch-callout:default}
@media(any-pointer:coarse){.copy-text-button{min-height:44px;min-width:80px;font-size:13px}}
</style>
