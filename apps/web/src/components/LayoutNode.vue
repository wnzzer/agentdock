<script setup lang="ts">
import { computed, nextTick, onMounted, onBeforeUnmount, ref, watch } from "vue";
import type { LayoutNode, LeafPaneKind, PaneNode, SplitDirection } from "@agentdock/protocol/layout";
import type { ProviderKind } from "@agentdock/protocol";
import { isPaneNode, normalizeRatio, snapRatio, SPLIT_GAP, type DockPosition } from "../layout/layout-engine";
import { useI18n } from "../i18n";
import TabIcon from "../features/TabIcon.vue";
import Icon from "../features/Icon.vue";
const { t } = useI18n();

const props = defineProps<{ node: LayoutNode; selected?: string | null; maximized?: string | null; locatedPaneId?: string | null; workspaceLabels?: Record<string, string>; workspaceBranches?: Record<string, string>; sessionBranches?: Record<string, string>; sessionProviders?: Record<string, ProviderKind>; ephemeralSessionIds?: string[]; ephemeralSupported?: boolean }>();
defineSlots<{ pane(props: { pane: PaneNode }): unknown }>();
const emit = defineEmits<{
  select: [pane: PaneNode, groupId?: string];
  split: [id: string, direction: SplitDirection];
  resize: [id: string, ratio: number, final: boolean];
  resizing: [active: boolean];
  close: [id: string];
  maximize: [id: string];
  "drop-pane": [targetId: string, pane: PaneNode, position: DockPosition];
  "add-pane": [targetId: string, kind: LeafPaneKind];
  "create-session": [targetId: string, provider: ProviderKind, ephemeral: boolean];
  "reveal-session": [sessionId: string];
}>();

const splitElement = ref<HTMLElement | null>(null);
const paneElement = ref<HTMLElement | null>(null);
const dropPosition = ref<DockPosition | null>(null);
const addMenu = ref<HTMLDetailsElement | null>(null);
/**
 * Tab context menu.
 *
 * Closing tabs one cross at a time is the tedious part of a busy layout, so
 * the bulk actions live here. Each one resolves to a list of panes first and
 * then closes them, because the tab list changes as they go.
 */
const tabMenu = ref<{ pane: PaneNode; x: number; y: number } | null>(null);
function openTabMenu(event: MouseEvent, pane: PaneNode) {
  event.preventDefault();
  tabMenu.value = { pane, x: event.clientX, y: event.clientY };
}
function closeTabMenu() { tabMenu.value = null; }
const tabMenuStyle = computed(() => tabMenu.value
  // Kept inside the viewport: a tab near the right or bottom edge would
  // otherwise open a menu that runs off the screen.
  ? { left: Math.min(tabMenu.value.x, window.innerWidth - 190) + 'px', top: Math.min(tabMenu.value.y, window.innerHeight - 180) + 'px' }
  : {});
const menuOthers = computed(() => tabMenu.value ? tabs.value.filter(pane => pane.id !== tabMenu.value!.pane.id) : []);
const menuRight = computed(() => {
  const current = tabMenu.value; if (!current) return [];
  const index = tabs.value.findIndex(pane => pane.id === current.pane.id);
  return index < 0 ? [] : tabs.value.slice(index + 1);
});
function closePanes(panes: PaneNode[]) {
  closeTabMenu();
  for (const pane of [...panes]) emit("close", pane.id);
}
const activePane = computed(() => {
  const node = props.node;
  return node.type === "pane" ? node : node.type === "stack" ? node.panes.find((pane) => pane.id === node.activePaneId) ?? node.panes[0] : undefined;
});
const tabs = computed(() => props.node.type === "pane" ? [props.node] : props.node.type === "stack" ? props.node.panes : []);
async function revealActiveTab() {
  await nextTick();
  const active = paneElement.value?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]');
  const strip = active?.parentElement;
  if (!active || !strip) return;
  const bounds = active.getBoundingClientRect(), viewport = strip.getBoundingClientRect();
  if (bounds.right > viewport.right) strip.scrollLeft += bounds.right - viewport.right;
  if (bounds.left < viewport.left) strip.scrollLeft -= viewport.left - bounds.left;
}
watch(() => [activePane.value?.id, tabs.value.map(pane => pane.id).join(',')], revealActiveTab);
onMounted(() => { void revealActiveTab(); window.addEventListener('resize', revealActiveTab); });
const targetId = computed(() => props.node.type === "stack" ? props.node.collapsedFrom ?? props.node.id : props.node.id);
const splitStyle = computed(() => {
  if (props.node.type !== "split") return {};
  const ratio = normalizeRatio(props.node.ratio);
  const tracks = `minmax(0, ${ratio}fr) ${SPLIT_GAP}px minmax(0, ${1 - ratio}fr)`;
  return props.node.direction === "horizontal" ? { gridTemplateColumns: tracks, gridTemplateRows: "minmax(0, 1fr)" } : { gridTemplateRows: tracks, gridTemplateColumns: "minmax(0, 1fr)" };
});
/**
 * Views that are meaningful while still empty. A session pane is not one of
 * them: an agent tab bound to no session is a dead placeholder, and "+" now
 * consistently means "new session" — created through the entries above these.
 */
const paneTypes: { kind: LeafPaneKind; title: string }[] = [
  { kind: "git_diff", title: "Git Changes" },
  { kind: "editor", title: "File editor" },
  { kind: "file_preview", title: "File preview" },
];
const newSessionKinds: { provider: ProviderKind; title: string; ephemeral?: boolean }[] = [
  { provider: "claude_code", title: "New Claude Code session" },
  { provider: "codex", title: "New Codex session" },
  { provider: "terminal", title: "New terminal" },
];
function titleFor(pane: PaneNode) {
  // Persist canonical built-in titles, not the chosen display language. Native
  // session/file titles may coincide with UI copy and must remain verbatim.
  if (pane.title && (pane.metadata?.session_id || pane.metadata?.path)) return pane.title;
  return t(pane.title || paneTypes.find((entry) => entry.kind === pane.kind)?.title || pane.kind);
}
/** A session's branch is its workspace's; files and Git panes already say
 * which checkout they show, so only sessions carry it. */
function branchFor(pane: PaneNode) {
  if (pane.kind !== "agent_chat" && pane.kind !== "terminal") return undefined;
  // A session in a worktree has a branch of its own; the rest share their workspace's.
  const session = pane.metadata?.session_id, id = pane.metadata?.workspace_id;
  return (typeof session === "string" ? props.sessionBranches?.[session] : undefined) ?? (typeof id === "string" ? props.workspaceBranches?.[id] : undefined);
}
function workspaceFor(pane: PaneNode) { const id = pane.metadata?.workspace_id; return typeof id === "string" ? props.workspaceLabels?.[id] : undefined; }
const ephemeralIds = computed(() => new Set(props.ephemeralSessionIds ?? []));
function isEphemeral(pane: PaneNode) { const id = pane.metadata?.session_id; return typeof id === "string" && ephemeralIds.value.has(id); }
function qualifiedTitle(pane: PaneNode) { const workspace = workspaceFor(pane); return titleFor(pane) + (workspace ? " · " + workspace : ""); }
function tabLabel(pane: PaneNode) { return isEphemeral(pane) ? t('{title} · temporary window', { title: qualifiedTitle(pane) }) : qualifiedTitle(pane); }
function select(pane: PaneNode) { emit("select", pane, props.node.type === "stack" ? props.node.collapsedFrom : undefined); }
function tabKey(event: KeyboardEvent, pane: PaneNode) {
  if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
  event.preventDefault();
  const index = tabs.value.findIndex((item) => item.id === pane.id);
  const nextIndex = event.key === "Home" ? 0 : event.key === "End" ? tabs.value.length - 1 : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.value.length) % tabs.value.length;
  const next = tabs.value[nextIndex];
  if (next) { select(next); paneElement.value?.querySelectorAll<HTMLElement>("[role=tab]")[nextIndex]?.focus(); }
}
function add(kind: LeafPaneKind) { if (addMenu.value) addMenu.value.open = false; emit("add-pane", targetId.value, kind); }
function createSession(provider: ProviderKind, ephemeral = false) { if (addMenu.value) addMenu.value.open = false; emit("create-session", targetId.value, provider, ephemeral); }
function forwardCreate(id: string, provider: ProviderKind, ephemeral: boolean) { emit("create-session", id, provider, ephemeral); }

// Vue emits forward positional arguments, not an $event array. Keep every
// recursive hop typed so nested operations address their original node IDs.
function forwardSelect(pane: PaneNode, groupId?: string) { emit("select", pane, groupId); }
function forwardSplit(id: string, direction: SplitDirection) { emit("split", id, direction); }
function forwardResize(id: string, ratio: number, final: boolean) { emit("resize", id, ratio, final); }
function forwardDrop(id: string, pane: PaneNode, position: DockPosition) { emit("drop-pane", id, pane, position); }
function forwardAdd(id: string, kind: LeafPaneKind) { emit("add-pane", id, kind); }

let cleanupResize: (() => void) | undefined;
function beginResize(event: PointerEvent) {
  if (props.node.type !== "split" || !splitElement.value || event.button !== 0) return;
  event.preventDefault();
  cleanupResize?.();
  const id = props.node.id;
  const horizontal = props.node.direction === "horizontal";
  const bounds = splitElement.value.getBoundingClientRect();
  const available = Math.max(1, (horizontal ? bounds.width : bounds.height) - SPLIT_GAP);
  const origin = horizontal ? bounds.left : bounds.top;
  let ratio = props.node.ratio;
  const previousCursor = document.body.style.cursor;
  const previousSelection = document.body.style.userSelect;
  document.body.style.cursor = horizontal ? "col-resize" : "row-resize";
  document.body.style.userSelect = "none";
  emit("resizing", true);
  const move = (next: PointerEvent) => {
    ratio = normalizeRatio(((horizontal ? next.clientX : next.clientY) - origin - SPLIT_GAP / 2) / available);
    emit("resize", id, ratio, false);
  };
  const cleanup = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", finish);
    document.body.style.cursor = previousCursor;
    document.body.style.userSelect = previousSelection;
    cleanupResize = undefined;
  };
  const finish = () => { cleanup(); emit("resize", id, snapRatio(ratio), true); emit("resizing", false); };
  cleanupResize = cleanup;
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", finish, { once: true });
  window.addEventListener("pointercancel", finish, { once: true });
}
function keyboardResize(event: KeyboardEvent) {
  if (props.node.type !== "split") return;
  const horizontal = props.node.direction === "horizontal";
  const decrease = event.key === (horizontal ? "ArrowLeft" : "ArrowUp");
  const increase = event.key === (horizontal ? "ArrowRight" : "ArrowDown");
  if (!decrease && !increase && event.key !== "Home") return;
  event.preventDefault();
  emit("resize", props.node.id, event.key === "Home" ? 0.5 : normalizeRatio(props.node.ratio + (increase ? 1 : -1) * (event.shiftKey ? 0.1 : 0.025)), true);
}
function dragStart(event: DragEvent, pane: PaneNode) {
  if (!event.dataTransfer) return;
  event.dataTransfer.effectAllowed = "move";
  event.dataTransfer.setData("application/agentdock-pane", JSON.stringify(pane));
}
function isPaneDrag(event: DragEvent) { return event.dataTransfer?.types.includes("application/agentdock-pane"); }
function detectPosition(event: DragEvent): DockPosition {
  const bounds = paneElement.value?.getBoundingClientRect();
  if (!bounds) return "center";
  const x = event.clientX - bounds.left;
  const y = event.clientY - bounds.top;
  const marginX = Math.min(76, bounds.width * 0.23);
  const marginY = Math.min(58, bounds.height * 0.23);
  const candidates: [DockPosition, number][] = [["left", x / marginX], ["right", (bounds.width - x) / marginX], ["top", y / marginY], ["bottom", (bounds.height - y) / marginY]];
  const nearest = candidates.sort((a, b) => a[1] - b[1])[0];
  return nearest[1] < 1 ? nearest[0] : "center";
}
function dragOver(event: DragEvent) {
  if (!isPaneDrag(event)) return;
  event.preventDefault();
  event.stopPropagation();
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = ["copy", "copyLink"].includes(event.dataTransfer.effectAllowed) ? "copy" : "move";
  }
  dropPosition.value = detectPosition(event);
}
function dragLeave(event: DragEvent) {
  if (event.relatedTarget instanceof Node && paneElement.value?.contains(event.relatedTarget)) return;
  dropPosition.value = null;
}
function drop(event: DragEvent) {
  if (!isPaneDrag(event)) return;
  event.preventDefault();
  event.stopPropagation();
  const position = dropPosition.value ?? detectPosition(event);
  dropPosition.value = null;
  const raw = event.dataTransfer?.getData("application/agentdock-pane");
  if (!raw || raw.length > 32768) return;
  try { const pane: unknown = JSON.parse(raw); if (isPaneNode(pane)) emit("drop-pane", targetId.value, pane, position); } catch { /* Foreign or malformed drag payload. */ }
}
onBeforeUnmount(() => { cleanupResize?.(); window.removeEventListener('resize', revealActiveTab); });
</script>

<template>
  <div v-if="node.type === 'split'" ref="splitElement" class="dock-split" :class="'is-' + node.direction" :style="splitStyle" :data-split-id="node.id">
    <div class="dock-child">
      <LayoutNode :node="node.first" :selected="selected" :maximized="maximized" :located-pane-id="locatedPaneId" :workspace-labels="workspaceLabels" :workspace-branches="workspaceBranches" :session-branches="sessionBranches" :session-providers="sessionProviders" :ephemeral-session-ids="ephemeralSessionIds" :ephemeral-supported="ephemeralSupported" @select="forwardSelect" @split="forwardSplit" @resize="forwardResize" @resizing="emit('resizing', $event)" @close="emit('close', $event)" @maximize="emit('maximize', $event)" @drop-pane="forwardDrop" @add-pane="forwardAdd" @create-session="forwardCreate" @reveal-session="emit('reveal-session', $event)">
        <template #pane="scope"><slot name="pane" :pane="scope.pane" /></template>
      </LayoutNode>
    </div>
    <div class="dock-separator" role="separator" tabindex="0" :aria-label="t(node.direction === 'horizontal' ? 'Resize columns' : 'Resize rows')" :aria-orientation="node.direction === 'horizontal' ? 'vertical' : 'horizontal'" :aria-valuenow="Math.round(node.ratio * 100)" :aria-valuemin="5" :aria-valuemax="95" :title="t('Drag to resize · double-click for 1:1 · arrow keys for precision')" @pointerdown="beginResize" @keydown="keyboardResize" @dblclick="emit('resize', node.id, 0.5, true)" />
    <div class="dock-child">
      <LayoutNode :node="node.second" :selected="selected" :maximized="maximized" :located-pane-id="locatedPaneId" :workspace-labels="workspaceLabels" :workspace-branches="workspaceBranches" :session-branches="sessionBranches" :session-providers="sessionProviders" :ephemeral-session-ids="ephemeralSessionIds" :ephemeral-supported="ephemeralSupported" @select="forwardSelect" @split="forwardSplit" @resize="forwardResize" @resizing="emit('resizing', $event)" @close="emit('close', $event)" @maximize="emit('maximize', $event)" @drop-pane="forwardDrop" @add-pane="forwardAdd" @create-session="forwardCreate" @reveal-session="emit('reveal-session', $event)">
        <template #pane="scope"><slot name="pane" :pane="scope.pane" /></template>
      </LayoutNode>
    </div>
  </div>
  <section v-else ref="paneElement" class="dock-pane" :class="{ 'is-selected': activePane?.id === selected, 'is-collapsed': node.type === 'stack' && node.collapsedFrom, 'is-located': !!locatedPaneId && activePane?.id === locatedPaneId }" :data-pane-id="activePane?.id" :data-node-id="targetId" @pointerdown="activePane && select(activePane)" @dragover="dragOver" @dragleave="dragLeave" @drop="drop">
    <header class="dock-header">
      <div class="dock-tabs" role="tablist" :aria-label="t('Pane tabs')">
        <div v-for="pane in tabs" :key="pane.id" :id="`dock-tab-${pane.id}`" :data-pane-tab-id="pane.id" role="tab" class="dock-tab" :class="{ 'is-active': pane.id === activePane?.id, 'is-ephemeral': isEphemeral(pane) }" :aria-selected="pane.id === activePane?.id" :aria-controls="`dock-panel-${pane.id}`" :aria-label="tabLabel(pane)" :tabindex="pane.id === activePane?.id ? 0 : -1" :title="t(isEphemeral(pane) ? '{title} · temporary window · discarded when closed' : '{title} · drag to arrange', { title: qualifiedTitle(pane) })" draggable="true" @contextmenu="openTabMenu($event, pane)" @dragstart="dragStart($event, pane)" @click.stop="select(pane)" @keydown="tabKey($event, pane)" @keydown.enter.prevent="select(pane)" @keydown.space.prevent="select(pane)">
          <TabIcon :kind="pane.kind" :metadata="pane.metadata" :session-providers="sessionProviders" />
          <span v-if="isEphemeral(pane)" class="dock-tab-ephemeral" role="img" :aria-label="t('Temporary window')" :title="t('Temporary window')" />
          <span class="dock-tab-title">{{ titleFor(pane) }}</span>
          <span v-if="workspaceFor(pane)" class="dock-tab-workspace">{{ workspaceFor(pane) }}</span><span v-if="branchFor(pane)" class="dock-tab-branch" :title="branchFor(pane)"><svg viewBox="0 0 16 16" width="9" height="9" aria-hidden="true"><path d="M5 3v7M5 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm0-7a2 2 0 1 0 0-.01M11 5a2 2 0 1 0 0-.01M11 7c0 2-2 3-6 3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>{{ branchFor(pane) }}</span>
          <!-- The session menu is teleported here by the active session pane.
               Inactive tabs stay compact and do not reserve an empty action slot. -->
          <!-- Keep every session target mounted. A Teleport menu can outlive a
               tab activation during layout updates; removing its target in
               that frame causes Vue to patch a null vnode. -->
          <!-- Finding a session in the list is a navigation action, not a rare
               one, so it sits on the tab rather than behind the ··· menu. Only
               the active tab shows it, like the menu beside it. -->
          <button v-if="pane.metadata?.session_id && pane.id === activePane?.id" class="dock-tab-locate" type="button" :aria-label="t('Locate {title} in session list', { title: titleFor(pane) })" :title="t('Locate in session list')" @pointerdown.stop @click.stop="emit('reveal-session', String(pane.metadata.session_id))"><Icon name="locate" :size="12" /></button>
          <span v-if="pane.metadata?.session_id" class="dock-tab-session-actions" :id="`session-actions-${pane.id}`" />
          <button class="dock-tab-close" type="button" :aria-label="t('Close {title} pane', { title: tabLabel(pane) })" :title="t(isEphemeral(pane) ? 'Close and discard this temporary session' : 'Close pane (session keeps running)')" @pointerdown.stop @click.stop="emit('close', pane.id)"><Icon name="close" :size="12" /></button>
        </div>
        <span v-if="!tabs.length" class="dock-empty-label">{{ t('Empty pane') }}</span>
        <Teleport to="body"><div v-if="tabMenu" class="dock-tab-menu-backdrop" @pointerdown="closeTabMenu" @contextmenu.prevent="closeTabMenu"><nav class="dock-tab-menu" :style="tabMenuStyle" role="menu" :aria-label="t('Tab actions')" @pointerdown.stop @keydown.esc.stop.prevent="closeTabMenu"><button type="button" role="menuitem" @click="closePanes([tabMenu.pane])">{{ t('Close tab') }}</button><button type="button" role="menuitem" :disabled="!menuOthers.length" @click="closePanes(menuOthers)">{{ t('Close other tabs') }}</button><button type="button" role="menuitem" :disabled="!menuRight.length" @click="closePanes(menuRight)">{{ t('Close tabs to the right') }}</button><button type="button" role="menuitem" @click="closePanes(tabs)">{{ t('Close all tabs') }}</button><hr/><button type="button" role="menuitem" @click="closeTabMenu(); emit('maximize', tabMenu!.pane.id)">{{ t(maximized === tabMenu.pane.id ? 'Restore layout' : 'Maximize pane') }}</button><button type="button" role="menuitem" @click="closeTabMenu(); emit('split', tabMenu!.pane.id, 'horizontal')">{{ t('Split side by side') }}</button><button type="button" role="menuitem" @click="closeTabMenu(); emit('split', tabMenu!.pane.id, 'vertical')">{{ t('Split top and bottom') }}</button></nav></div></Teleport>
      </div>
      <div class="dock-actions" @pointerdown.stop>
        <button v-if="node.type === 'stack' && node.collapsedFrom" type="button" class="dock-action dock-adaptive" :title="t('Reset folded split to 1:1; expands when space allows')" :aria-label="t('Restore folded split ratio')" @click="emit('resize', targetId, 0.5, true)"><Icon name="restore" :size="13" /></button>
        <details ref="addMenu" class="dock-add-menu">
          <summary class="dock-action" :title="t('New session here')" :aria-label="t('New session here')"><Icon name="plus" :size="15" /></summary>
          <div class="dock-menu-items">
            <button v-for="entry in newSessionKinds" :key="entry.provider" type="button" @click="createSession(entry.provider)"><TabIcon :kind="entry.provider === 'terminal' ? 'terminal' : 'agent_chat'" :provider="entry.provider === 'terminal' ? undefined : entry.provider" :size="14" />{{ t(entry.title) }}</button>
            <button v-if="ephemeralSupported" type="button" @click="createSession('claude_code', true)"><Icon name="clock" :size="14" />{{ t('New temporary window') }}</button>
            <hr />
            <button v-for="type in paneTypes" :key="type.kind" type="button" @click="add(type.kind)"><TabIcon :kind="type.kind" :size="14" />{{ t(type.title) }}</button>
          </div>
        </details>
        <button class="dock-action" type="button" :aria-label="t('Split side by side')" :title="t('Split side by side')" @click="emit('split', targetId, 'horizontal')"><Icon name="splitHorizontal" :size="15" /></button>
        <button class="dock-action" type="button" :aria-label="t('Split top and bottom')" :title="t('Split top and bottom')" @click="emit('split', targetId, 'vertical')"><Icon name="splitVertical" :size="15" /></button>
        <button v-if="activePane" class="dock-action" type="button" :aria-label="t(maximized === activePane.id ? 'Restore pane' : 'Maximize pane')" :title="t(maximized === activePane.id ? 'Restore layout' : 'Maximize pane')" @click="emit('maximize', activePane.id)"><Icon :name="maximized === activePane.id ? 'minimize' : 'maximize'" :size="14" /></button>
        <button v-else class="dock-action" type="button" :aria-label="t('Close empty pane')" :title="t('Close empty pane')" @click="emit('close', targetId)"><Icon name="close" :size="14" /></button>
      </div>
    </header>
    <div v-if="activePane" :id="`dock-panel-${activePane.id}`" :aria-labelledby="`dock-tab-${activePane.id}`" class="dock-content" role="tabpanel"><slot name="pane" :pane="activePane" /></div>
    <div v-else class="dock-empty">
      <span class="dock-empty-symbol"><Icon name="layout" :size="28" /></span><strong>{{ t('A little room for your next idea') }}</strong><p>{{ t('Drop a session here, or add a pane.') }}</p>
      <div class="dock-empty-choices"><button v-for="type in paneTypes" :key="type.kind" type="button" @click="add(type.kind)"><TabIcon :kind="type.kind" :size="14" />{{ t(type.title) }}</button></div>
    </div>
    <div v-if="dropPosition" class="dock-drop-overlay" :class="'drop-' + dropPosition"><span>{{ t(dropPosition === 'center' ? 'Add as tab' : 'Split ' + dropPosition) }}</span></div>
  </section>
</template>

<style scoped>
/* The menu is teleported to the body so a tab strip with overflow clipping
   cannot cut it off, and the backdrop makes any click elsewhere dismiss it. */
.dock-tab-locate{flex:none;display:grid;place-items:center;width:18px;height:18px;padding:0;border:0;border-radius:5px;background:none;color:#7f93a1;cursor:pointer}
.dock-tab-locate:hover,.dock-tab-locate:focus-visible{color:var(--teal);background:var(--teal-soft)}
.dock-tab-menu-backdrop{position:fixed;inset:0;z-index:60}
.dock-tab-menu{position:fixed;min-width:180px;padding:5px;background:var(--surface);border:1px solid var(--border);border-radius:11px;box-shadow:0 14px 38px #243b4c2b}
.dock-tab-menu button{display:block;width:100%;min-height:32px;padding:7px 10px;border:0;border-radius:7px;background:none;text-align:left;font-size:12px;color:var(--ink-soft);white-space:nowrap;cursor:pointer}
.dock-tab-menu button:hover:not(:disabled){background:var(--teal-soft);color:var(--teal)}
.dock-tab-menu button:disabled{opacity:.4;cursor:not-allowed}
.dock-tab-menu hr{border:0;border-top:1px solid var(--border);margin:4px 6px}
@media(pointer:coarse){.dock-tab-menu button{min-height:44px}}
.dock-split,.dock-child { width:100%;height:100%;min-width:0;min-height:0; }
.dock-split { display:grid; }
.dock-child { overflow:hidden; }
.dock-separator { position:relative;outline:none;touch-action:none;z-index:2; }
.is-horizontal>.dock-separator { cursor:col-resize; }
.is-vertical>.dock-separator { cursor:row-resize; }
.dock-separator::after { content:"";position:absolute;border-radius:4px;inset:1px;background:transparent;transition:background .12s; }
.dock-separator:hover::after,.dock-separator:focus-visible::after { background:#51b9b0; }
.dock-pane { position:relative;display:flex;flex-direction:column;min-width:0;min-height:0;width:100%;height:100%;overflow:hidden;border:1px solid #dfe5e9;border-radius:10px;background:#fff;box-sizing:border-box;box-shadow:0 2px 8px #263a4910;container:dock-pane / inline-size; }
.dock-pane.is-selected { border-color:#8bcac4;box-shadow:0 0 0 1px #b6e3dd60,0 2px 8px #263a4910; }
.dock-pane.is-located { border-color:#269e8d;box-shadow:inset 0 0 0 1px #46b29d,0 0 0 1px #b6e3dd60;animation:dock-locate 1.4s ease-out; }.dock-pane.is-located .dock-tab.is-active { background:#e7f4f0; }
@keyframes dock-locate { from { box-shadow:inset 0 0 0 3px #70cabb,0 0 0 1px #b6e3dd60; } to { box-shadow:inset 0 0 0 1px #46b29d,0 0 0 1px #b6e3dd60; } }
@media(prefers-reduced-motion:reduce) { .dock-pane.is-located { animation:none; } }
.dock-header { display:flex;align-items:center;gap:3px;min-width:0;height:39px;min-height:39px;padding:0 5px 0 0;background:#fafbfc;border-bottom:1px solid #e8ecef; }
.dock-tabs { display:flex;align-items:stretch;flex:1;min-width:0;height:100%;overflow-x:auto; }
/* Tabs are reached by clicking, not by dragging a bar, so this one only has to
   show that more exist. A full-width scrollbar would take a quarter of the strip. */
.dock-tabs::-webkit-scrollbar { height: 5px; }
.dock-tabs::-webkit-scrollbar-thumb { border-width: 1px; }
.dock-tab { display:flex;align-items:center;gap:7px;max-width:210px;min-width:85px;flex-shrink:0;padding:0 8px 0 11px;color:#78838e;font-size:11px;cursor:grab;border-bottom:2px solid transparent;outline:none;user-select:none; }
.dock-tab.is-active { max-width:360px;color:#243746;background:#fff;border-bottom-color:#16a398; }
.dock-tab:focus-visible { box-shadow:inset 0 0 0 2px #53b9b0; }
.dock-tab-title { min-width:48px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-weight:600; }
.dock-tab-branch { display:inline-flex;align-items:center;gap:3px;max-width:80px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex-shrink:0;font-size:8px;border-radius:4px;padding:1px 4px;background:#efeafb;color:#7a66b0; }
.dock-tab-workspace { max-width:65px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex-shrink:0;font-size:8px;border-radius:4px;padding:1px 4px;background:#eaf2ed;color:#789b88; }
.dock-tab.is-ephemeral { border-bottom-style:dashed; }.dock-tab.is-ephemeral.is-active { border-bottom-color:#8973b4; }
.dock-tab-ephemeral { flex-shrink:0;width:5px;height:5px;margin-left:-3px;border-radius:50%;background:#8973b4;box-shadow:0 0 0 2px #efeaf8; }
.dock-tab-session-actions{position:relative;display:inline-flex;align-items:center;justify-content:center;width:28px;height:28px;flex:none}
.dock-tab:not(.is-active)>.dock-tab-session-actions { display:none; }
.dock-tab-close { display:grid;place-items:center;border:0;padding:0;background:transparent;width:17px;height:19px;color:#98a2aa;cursor:pointer;border-radius:4px;font-size:14px;flex-shrink:0; }
.dock-tab-close:hover { color:#be4453;background:#fff0f1; }
.dock-actions { display:flex;align-items:center;gap:1px;flex-shrink:0; }
.dock-action { display:grid;place-items:center;width:24px;height:25px;padding:0;border:0;border-radius:5px;background:transparent;color:#74818c;cursor:pointer;font-size:16px;list-style:none; }
.dock-action:hover,.dock-action:focus-visible { background:#eaf3f2;color:#087e73;outline:none; }
.dock-action::-webkit-details-marker { display:none; }
.dock-add-menu { position:static; }.dock-menu-items { position:absolute;right:6px;top:37px;z-index:10;min-width:154px;background:#fff;border:1px solid #dce4e8;border-radius:8px;padding:5px;box-shadow:0 8px 22px #1e334222; }
.dock-menu-items button { display:flex;align-items:center;gap:9px;width:100%;border:0;border-radius:5px;padding:8px;background:transparent;color:#485b68;text-align:left;font-size:12px;cursor:pointer; }.dock-menu-items button:hover { background:#ecf7f5;color:#087e73; }
.dock-adaptive { color:#a1aaaF;font-size:12px;padding:0 3px; }
.dock-content { display:flex;flex-direction:column;flex:1;min-width:0;min-height:0;overflow:hidden; }
.dock-content :deep(> *) { min-width:0;min-height:0; }
.dock-empty-label { padding:12px;color:#9aa4ad;font-size:11px; }
.dock-empty { flex:1;min-height:0;overflow:auto;display:flex;align-items:center;justify-content:center;flex-direction:column;padding:20px;color:#758590;text-align:center; }
.dock-empty-symbol { color:#9ab7bb;font-size:30px;margin-bottom:10px; }.dock-empty strong { font-size:13px;font-weight:500;color:#5b707d; }.dock-empty p { font-size:11px;margin:6px 0 17px; }
.dock-empty-choices { display:flex;flex-wrap:wrap;justify-content:center;gap:6px;max-width:350px; }.dock-empty-choices button { display:flex;align-items:center;gap:6px;border:1px solid #e1e9ec;border-radius:6px;padding:6px 9px;background:#fff;font-size:11px;color:#607583;cursor:pointer; }.dock-empty-choices button:hover { background:#eff9f7;border-color:#afdad3; }
.dock-drop-overlay { position:absolute;inset:5px;z-index:20;display:flex;align-items:center;justify-content:center;pointer-events:none;border:2px solid #24a69a;border-radius:8px;background:#b9eae280;color:#086d63;backdrop-filter:blur(1px);font-size:12px;font-weight:600; }.dock-drop-overlay span { border-radius:20px;padding:7px 12px;background:#fffffff0;box-shadow:0 2px 8px #27655a15; }.drop-left { right:50%; }.drop-right { left:50%; }.drop-top { bottom:50%; }.drop-bottom { top:50%; }
@container dock-pane (max-width:480px) { .dock-tab-workspace,.dock-tab-branch { display:none; }.dock-tab,.dock-tab.is-active { max-width:180px; } }
@media(max-width:520px) { .dock-tab,.dock-tab.is-active { min-width:72px;max-width:180px;padding-left:7px;gap:5px; }.dock-tab-workspace,.dock-tab-branch { display:none; }.dock-action { width:23px; }.dock-adaptive { display:none; } }
</style>
