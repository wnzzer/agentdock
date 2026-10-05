<script setup lang="ts">
/**
 * A menu opened at the pointer by a right-click.
 *
 * Teleported to the body so no overflow clipping can cut it off, with a
 * backdrop so any press elsewhere -- another right-click included -- closes it.
 * Items are plain <button>s and <hr>s in the default slot; an item closes the
 * menu itself by calling the owner's close.
 */
import { computed, nextTick, onMounted, ref } from "vue";

const props = withDefaults(defineProps<{ x: number; y: number; label: string; width?: number }>(), { width: 200 });
const emit = defineEmits<{ close: [] }>();
const menu = ref<HTMLElement>();
const height = ref(0);
const style = computed(() => ({
  left: Math.max(4, Math.min(props.x, window.innerWidth - props.width - 8)) + "px",
  top: Math.max(4, Math.min(props.y, window.innerHeight - height.value - 8)) + "px",
  minWidth: props.width + "px",
}));
function items() { return Array.from(menu.value?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []); }
function move(event: KeyboardEvent) {
  if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
  event.preventDefault();
  const list = items(), index = list.indexOf(document.activeElement as HTMLButtonElement);
  list[(index + (event.key === "ArrowDown" ? 1 : -1) + list.length) % list.length]?.focus();
}
onMounted(async () => { await nextTick(); height.value = menu.value?.offsetHeight ?? 0; items()[0]?.focus({ preventScroll: true }); });
</script>

<template>
  <Teleport to="body">
    <div class="context-menu-backdrop" @pointerdown="emit('close')" @contextmenu.prevent="emit('close')">
      <nav ref="menu" class="context-menu" :style="style" role="menu" :aria-label="label" @pointerdown.stop @contextmenu.prevent.stop @keydown.esc.stop.prevent="emit('close')" @keydown="move"><slot /></nav>
    </div>
  </Teleport>
</template>

<style>
.context-menu-backdrop{position:fixed;inset:0;z-index:70}
.context-menu{position:fixed;max-height:calc(100vh - 16px);overflow:auto;padding:5px;background:var(--surface,var(--surface));border:1px solid var(--border,var(--line));border-radius:var(--radius-md);box-shadow:var(--shadow-lg)}
.context-menu>button{display:flex;align-items:center;gap:8px;width:100%;min-height:30px;padding:6px 10px;border:0;border-radius:var(--radius-sm);background:none;text-align:left;font-size:var(--text-sm);color:var(--ink-soft,var(--ink-soft));white-space:nowrap;cursor:pointer}
.context-menu>button>svg{flex:none;color:var(--violet)}
.context-menu>button:hover:not(:disabled),.context-menu>button:focus-visible{background:var(--fill);color:var(--ink);outline:none}
.context-menu>button:disabled{opacity:.4;cursor:not-allowed}
.context-menu>button.danger{color:var(--danger)}.context-menu>button.danger:hover:not(:disabled){background:var(--danger-soft);color:var(--danger)}
.context-menu>hr{border:0;border-top:1px solid var(--border,var(--fill-hover));margin:4px 6px}
.context-menu>.context-menu-label{padding:5px 10px 3px;font-size:var(--text-xs);font-weight:600;color:var(--faint)}
@media(pointer:coarse){.context-menu>button{min-height:44px}}
</style>
