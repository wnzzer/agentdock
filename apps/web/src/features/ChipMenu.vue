<script setup lang="ts">
/**
 * A compact labelled chip that opens a panel above itself.
 *
 * This exists because the composer mixed a native `<select>` with hand-built
 * pop-ups: the OS chrome never matched the row around it. Every chip-shaped
 * control in the app is this component, so they open, close and size alike.
 *
 * The panel is whatever the caller slots in — a list of choices, a slider, a
 * form. Dismissal (pointer outside, Escape, choosing something) is handled here
 * once rather than re-implemented per control.
 */
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue';
import { chipPanelOffset } from './chip-menu-position';
import Icon from './Icon.vue';

defineProps<{
  /** The chip's own text. Truncated rather than wrapped: the row is one line. */
  label: string;
  title?: string;
  /** Marks the control as carrying a deliberate, non-default choice. */
  active?: boolean;
  /** A chip whose current setting is worth being uneasy about. */
  tone?: 'danger';
  disabled?: boolean;
}>();
const root = ref<HTMLDetailsElement>();
const panel = ref<HTMLElement>();
/** Measured when it opens rather than guessed at from the chip's position: how
 * wide the panel is depends on what the caller slotted into it. */
const offset = ref(0);
defineExpose({ close });

async function place() {
  const menu = root.value;
  if (!menu?.open) return;
  offset.value = 0;
  await nextTick();
  const box = panel.value?.getBoundingClientRect();
  if (box) offset.value = chipPanelOffset(box.left, box.width, window.innerWidth);
}

function close(focus = false) {
  const menu = root.value;
  if (!menu?.open) return;
  menu.open = false;
  if (focus) menu.querySelector<HTMLElement>('summary')?.focus();
}
function onOutside(event: PointerEvent) {
  const menu = root.value;
  if (menu?.open && !menu.contains(event.target as Node)) menu.open = false;
}
/** A disabled chip must not open; `<summary>` has no disabled attribute. */
function guard(event: Event) {
  const menu = root.value;
  if (menu && !menu.open && menu.dataset.locked === 'true') event.preventDefault();
}
onMounted(() => { document.addEventListener('pointerdown', onOutside); window.addEventListener('resize', place); });
onBeforeUnmount(() => { document.removeEventListener('pointerdown', onOutside); window.removeEventListener('resize', place); });
</script>

<template>
  <details ref="root" class="chip-menu" :class="{ 'is-active': active, 'is-disabled': disabled, 'is-danger': tone === 'danger' }" :data-locked="disabled ? 'true' : 'false'" @keydown.esc.stop.prevent="close(true)" @toggle="place">
    <summary :title="title ?? label" :aria-disabled="disabled ? 'true' : undefined" @click="guard">
      <slot name="mark" />
      <span class="chip-menu-label">{{ label }}</span>
      <Icon class="chip-menu-caret" name="chevron" :size="12" />
    </summary>
    <div ref="panel" class="chip-menu-panel" :style="{ left: offset + 'px' }"><slot :close="close" /></div>
  </details>
</template>

<style scoped>
.chip-menu{position:relative;flex:0 1 auto;min-width:0}
/* The border is always there and usually invisible, so a chip that lights up
   does not shift the row by a pixel as it does. */
.chip-menu>summary{list-style:none;display:flex;align-items:center;gap:5px;min-height:28px;max-width:100%;padding:0 8px;border:1px solid transparent;border-radius:7px;background:var(--fill);font-size:11px;color:var(--ink-soft);cursor:pointer;white-space:nowrap}
.chip-menu>summary::-webkit-details-marker{display:none}
.chip-menu>summary:hover{background:var(--fill-hover)}
.chip-menu[open]>summary{background:#e2e8ec;color:#3d4f5c}
.chip-menu.is-disabled>summary{cursor:not-allowed;opacity:.55}
/* A set control is legible as set from across the row, not only once read:
   colour alone is a weak signal among four chips that otherwise match. */
.chip-menu.is-active>summary{color:var(--teal);font-weight:600;background:#e9f3f0;border-color:#c2ddd5}
.chip-menu.is-danger>summary,.chip-menu.is-danger.is-active>summary{color:#a85c4e;background:#fdf1ee;border-color:#e6b5ad}
.chip-menu.is-danger[open]>summary{background:#fbe8e3;color:#8f4b3e}
/* The mark names the control; the label says what it is set to. Together they
   are what tells four chips apart at a glance. */
.chip-menu>summary>svg{flex:0 0 auto;opacity:.8}
.chip-menu.is-active>summary>svg,.chip-menu.is-danger>summary>svg{opacity:1}
.chip-menu-label{overflow:hidden;text-overflow:ellipsis}
.chip-menu-caret{transform:rotate(90deg);flex:0 0 auto;opacity:.55}
.chip-menu[open] .chip-menu-caret{transform:rotate(-90deg)}
.chip-menu-panel{position:absolute;bottom:calc(100% + 8px);left:0;z-index:20;background:var(--surface);border:1px solid var(--border);border-radius:13px;box-shadow:0 12px 35px #243b4c24}
/* Density follows the pointing device, not the pane width: a narrow pane on a
   desktop is still a mouse. */
@media(pointer:coarse){.chip-menu>summary{min-height:44px;border-radius:9px;padding:0 10px;font-size:11px}}
</style>
