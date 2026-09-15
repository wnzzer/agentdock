<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import Icon from "./Icon.vue";
import { useI18n } from "../i18n";
const { t } = useI18n();
/** `embedded` renders the content alone, for a dialog shown as one page inside
 * another dialog. The outer shell then owns the backdrop, heading and focus. */
const props = withDefaults(defineProps<{ title: string; wide?: boolean; closable?: boolean; embedded?: boolean }>(), { closable: true });
const emit = defineEmits<{ close: [] }>();
const dialog = ref<HTMLElement>();
let previousFocus: HTMLElement | null = null;
function onKey(event: KeyboardEvent) {
  if (event.key === "Escape" && props.closable) { event.preventDefault(); emit("close"); }
  if (event.key !== "Tab") return;
  const targets = Array.from(dialog.value?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href]') ?? []).filter(el => el.offsetParent !== null);
  const first = targets[0], last = targets[targets.length - 1];
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
onMounted(() => { if (props.embedded) return; previousFocus = document.activeElement as HTMLElement; dialog.value?.querySelector<HTMLElement>('[autofocus], input, button')?.focus(); });
onUnmounted(() => { if (!props.embedded) previousFocus?.focus(); });
</script>
<template>
  <div v-if="embedded" class="dialog-content embedded"><slot /></div>
  <Teleport v-else to="body"><div class="modal-backdrop" @mousedown.self="closable && emit('close')"><section ref="dialog" :class="['dialog', { wide }]" role="dialog" aria-modal="true" :aria-label="title" @keydown="onKey"><header class="dialog-header"><h2>{{ title }}</h2><button v-if="closable" class="icon-button" :aria-label="t('Close {title}', { title })" @click="emit('close')"><Icon name="close" /></button></header><div class="dialog-content"><slot /></div></section></div></Teleport>
</template>
<style>.dialog-content.embedded{padding:0;overflow:visible}</style>
