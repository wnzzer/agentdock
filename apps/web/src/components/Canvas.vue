<script setup lang="ts">
/**
 * Workspace Canvas API:
 *   <Canvas v-model="layout" :selected-pane-id="selected" @select-pane="select">
 *     <template #pane="{ pane }"><YourRealPane :pane="pane" /></template>
 *   </Canvas>
 * expose openPane(PaneNode) to open/select sessions and files from the sidebar.
 * focusPane(id) locates an existing pane without opening or starting a session.
 * Drag MIME application/agentdock-pane is a JSON PaneNode, e.g.
 * {type:'pane',id:'session-123',kind:'agent_chat',title:'Refactor',metadata:{session_id:'123'}}.
 * update:modelValue always emits CANONICAL data; debounce network persistence
 * in the parent. Responsive tabs and maximization never mutate saved geometry.
 * Pane content/lifetime belongs to the slot owner; closing is view-only.
 */
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import type { LayoutDocument, LayoutNode as Node, LeafPaneKind, PaneNode, SplitDirection } from "@agentdock/protocol/layout";
import type { ProviderKind } from "@agentdock/protocol";
import LayoutNode from "./LayoutNode.vue";
import Icon from "../features/Icon.vue";
import { activatePane, applyPreset, closePane as removePane, containerId, createPane, dockPane, findNode, findNodes, flattenPanes, projectLayout, replacePane, resizeSplit, restoreCollapsed, selectionGroups, splitPane, updatePane, validateLayout, type DockPosition, type LayoutPreset } from "../layout/layout-engine";
import { useI18n } from "../i18n";
const { t } = useI18n();

const props = withDefaults(defineProps<{
  modelValue: LayoutDocument;
  selectedPaneId?: string | null;
  minWidth?: number;
  minHeight?: number;
  defaultWorkspaceId?: string;
  workspaceLabels?: Record<string, string>;
  sessionProviders?: Record<string, ProviderKind>;
  ephemeralSessionIds?: string[];
  ephemeralSupported?: boolean;
  acceptPane?: (pane: PaneNode) => boolean;
  /** Owner veto for a close gesture. Layout still never ends a session by
   * itself; the owner decides whether a bound resource may be released first. */
  confirmClosePane?: (pane: PaneNode) => boolean | Promise<boolean>;
}>(), { minWidth: 280, minHeight: 180 });
const emit = defineEmits<{ "update:modelValue": [document: LayoutDocument]; "select-pane": [pane: PaneNode]; "open-pane": [pane: PaneNode]; "create-session": [targetId: string, provider: ProviderKind, ephemeral: boolean]; "reveal-session": [sessionId: string] }>();
defineSlots<{ pane(props: { pane: PaneNode }): unknown }>();
const working = shallowRef<LayoutDocument>(restoreCollapsed(props.modelValue));
const stage = ref<HTMLElement | null>(null);
const layoutMenu = ref<HTMLDetailsElement | null>(null);
const dimensions = ref({ width: 0, height: 0 });
const selected = ref<string | null>(props.selectedPaneId ?? null);
const maximized = ref<string | null>(null);
const resizing = ref(false);
const activeByGroup = ref<Record<string, string>>({});
const notice = ref("");
const locatedPaneId = ref<string | null>(null);
let observer: ResizeObserver | undefined;
let locateTimer: ReturnType<typeof setTimeout> | undefined;
let locateRequest = 0;
let generatedId = 0;

watch(() => props.modelValue, (value) => {
  if (!validateLayout(value)) { notice.value = "This layout is invalid. Your current workspace has been kept."; return; }
  working.value = restoreCollapsed(value);
  const panes = flattenPanes(working.value.root);
  if (selected.value && !panes.some((pane) => pane.id === selected.value)) selected.value = panes[0]?.id ?? null;
  if (maximized.value && !panes.some((pane) => pane.id === maximized.value)) maximized.value = null;
});
watch(() => props.selectedPaneId, (id) => {
  if (!id) return;
  const pane = findNode(working.value.root, id);
  if (pane?.type === "pane") selectPane(pane);
}, { immediate: true });

const panes = computed(() => flattenPanes(working.value.root));
const projected = computed(() => dimensions.value.width > 0 && dimensions.value.height > 0 && !resizing.value ? projectLayout(working.value.root, dimensions.value.width, dimensions.value.height, {
  minWidth: props.minWidth, minHeight: props.minHeight, selectedPaneId: selected.value, activeByGroup: activeByGroup.value,
}) : working.value.root);
const visible = computed(() => {
  const focused = maximized.value ? findNode(working.value.root, maximized.value) : undefined;
  return focused?.type === "pane" ? focused : projected.value;
});
const collapsedCount = computed(() => findNodes(projected.value, (node) => node.type === "stack" && !!node.collapsedFrom).length);

function commit(root: Node) {
  if (root === working.value.root) return;
  const next: LayoutDocument = { version: working.value.version, root };
  const result = validateLayout(next, true);
  if (!result.valid) { notice.value = result.errors[0] ?? "Layout change could not be applied."; return; }
  notice.value = "";
  working.value = next;
  emit("update:modelValue", next);
}
function selectPane(pane: PaneNode, groupId?: string) {
  selected.value = pane.id;
  const groups = selectionGroups(working.value.root, pane.id);
  if (groupId && !groups.includes(groupId)) groups.push(groupId);
  if (groups.length) activeByGroup.value = { ...activeByGroup.value, ...Object.fromEntries(groups.map((id) => [id, pane.id])) };
  commit(activatePane(working.value.root, pane.id));
  emit("select-pane", pane);
}
function split(id: string, direction: SplitDirection) {
  maximized.value = null;
  commit(splitPane(working.value.root, id, direction));
}
function resize(id: string, ratio: number, _final: boolean) { commit(resizeSplit(working.value.root, id, ratio)); }
async function close(id: string) {
  const target = findNode(working.value.root, id);
  // Ask before removing the view, so a refused release leaves the tab in place
  // rather than hiding a record that still exists on the server.
  if (props.confirmClosePane && target?.type === "pane" && !await props.confirmClosePane(target)) return;
  if (maximized.value === id) maximized.value = null;
  commit(removePane(working.value.root, id));
  if (selected.value === id) {
    const first = flattenPanes(working.value.root)[0];
    selected.value = first?.id ?? null;
    if (first) selectPane(first);
  }
}
function maximize(id: string) { maximized.value = maximized.value === id ? null : id; }
function preset(value: LayoutPreset) {
  closeLayoutMenu();
  maximized.value = null;
  commit(applyPreset(working.value.root, value, selected.value));
}
function closeLayoutMenu() { if (layoutMenu.value) layoutMenu.value.open = false; }
function layoutPointerDown(event: PointerEvent) {
  if (event.target instanceof globalThis.Node && !layoutMenu.value?.contains(event.target)) closeLayoutMenu();
}
function layoutEscape(event: KeyboardEvent) {
  if (event.key !== "Escape" || !layoutMenu.value?.open) return;
  event.preventDefault();
  event.stopPropagation();
  closeLayoutMenu();
  layoutMenu.value.querySelector<HTMLElement>("summary")?.focus();
}
function splitSelected(direction: SplitDirection) {
  closeLayoutMenu();
  const target = selected.value ?? panes.value[0]?.id ?? working.value.root.id;
  split(containerId(working.value.root, target), direction);
}
function restoreView() { closeLayoutMenu(); maximized.value = null; }

function centerTarget(id: string): string {
  const canonical = findNode(working.value.root, id);
  if (canonical?.type !== "split") return id;
  const projectedGroup = findNodes(projected.value, (node) => node.type === "stack" && node.collapsedFrom === id)[0];
  const activeId = projectedGroup?.type === "stack" ? projectedGroup.activePaneId : undefined;
  return activeId ? containerId(working.value.root, activeId) : id;
}
function drop(targetId: string, pane: PaneNode, position: DockPosition) {
  maximized.value = null;
  const target = position === "center" ? centerTarget(targetId) : targetId;
  commit(dockPane(working.value.root, pane, target, position));
  const inserted = findNode(working.value.root, pane.id);
  if (inserted?.type === "pane") selectPane(inserted);
}
function dropFromPointer(targetId: string, pane: PaneNode, position: DockPosition) {
  if (props.acceptPane && !props.acceptPane(pane)) { notice.value = "This pane does not match a known workspace or session."; return; }
  const existed = !!findNode(working.value.root, pane.id);
  drop(targetId, pane, position);
  const inserted = findNode(working.value.root, pane.id);
  // A successful new sidebar drop is an explicit open. Moving existing tabs is
  // only layout editing and must never restart an ended session.
  if (!existed && inserted?.type === "pane") emit("open-pane", inserted);
}
const titles: Record<LeafPaneKind, string> = { agent_chat: "Agent session", git_diff: "Changes", editor: "File editor", file_preview: "File preview", terminal: "Terminal" };
function add(targetId: string, kind: LeafPaneKind) {
  let id: string;
  do { id = `pane-${kind}-${Date.now().toString(36)}-${++generatedId}`; } while (findNode(working.value.root, id));
  const target = findNodes(projected.value, node => node.type === "stack" && node.collapsedFrom === targetId)[0] ?? findNode(working.value.root, targetId);
  const active = target?.type === "pane" ? target : target?.type === "stack" ? target.panes.find(pane => pane.id === target.activePaneId) ?? target.panes[0] : target ? flattenPanes(target)[0] : undefined;
  const workspaceId = typeof active?.metadata?.workspace_id === "string" ? active.metadata.workspace_id : props.defaultWorkspaceId;
  drop(targetId, createPane(id, kind, titles[kind], workspaceId ? { workspace_id: workspaceId } : undefined), "center");
}
/** Open into the stack the gesture came from, so "+" on a tab strip lands there. */
function openPaneAt(targetId: string | undefined, pane: PaneNode) {
  if (!targetId || findNode(working.value.root, pane.id) || !findNode(working.value.root, targetId)) { openPane(pane); return; }
  drop(targetId, pane, "center");
  selectPane(pane);
}
function openPane(pane: PaneNode) {
  const existing = findNode(working.value.root, pane.id);
  if (existing?.type === "pane") {
    commit(updatePane(working.value.root, pane));
    if (maximized.value) maximized.value = pane.id;
    selectPane(pane);
    return;
  }
  const reusable = pane.kind !== "git_diff" ? panes.value.find(candidate => candidate.kind === pane.kind && !candidate.metadata?.session_id && !candidate.metadata?.path && (!candidate.metadata?.workspace_id || candidate.metadata.workspace_id === pane.metadata?.workspace_id)) : undefined;
  if (reusable) {
    commit(replacePane(working.value.root, reusable.id, pane));
    if (maximized.value === reusable.id) maximized.value = pane.id;
    selectPane(pane);
    return;
  }
  const target = selected.value && findNode(working.value.root, selected.value) ? containerId(working.value.root, selected.value) : working.value.root.id;
  drop(target, pane, "center");
}
async function focusPane(id: string): Promise<boolean> {
  const pane = findNode(working.value.root, id);
  if (pane?.type !== "pane") return false;
  const request = ++locateRequest;
  clearTimeout(locateTimer);
  locatedPaneId.value = null;
  if (maximized.value) maximized.value = pane.id;
  selectPane(pane);
  await nextTick();
  if (request !== locateRequest) return true;
  const tab = Array.from(stage.value?.querySelectorAll<HTMLElement>("[data-pane-tab-id]") ?? []).find(element => element.dataset.paneTabId === id);
  tab?.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "instant" });
  tab?.focus({ preventScroll: true });
  locatedPaneId.value = id;
  locateTimer = setTimeout(() => { locatedPaneId.value = null; }, 1400);
  return true;
}
defineExpose({ openPane, openPaneAt, focusPane, closePane: close, maximizePane: maximize });

onMounted(() => {
  if (!selected.value && panes.value[0]) selected.value = panes.value[0].id;
  observer = new ResizeObserver(([entry]) => {
    if (entry) dimensions.value = { width: entry.contentRect.width, height: entry.contentRect.height };
  });
  if (stage.value) observer.observe(stage.value);
  document.addEventListener("pointerdown", layoutPointerDown);
});
onBeforeUnmount(() => {
  observer?.disconnect();
  clearTimeout(locateTimer);
  locateRequest++;
  document.removeEventListener("pointerdown", layoutPointerDown);
});
</script>

<template>
  <div class="dock-canvas" :class="{ 'is-resizing': resizing }">
    <div v-if="notice" class="dock-canvas-notice" role="status">{{ t(notice) }}<button type="button" :aria-label="t('Dismiss layout notice')" @click="notice = ''">×</button></div>
    <div ref="stage" class="dock-canvas-stage">
      <LayoutNode :node="visible" :selected="selected" :maximized="maximized" :located-pane-id="locatedPaneId" :workspace-labels="workspaceLabels" :session-providers="sessionProviders" :ephemeral-session-ids="ephemeralSessionIds" :ephemeral-supported="ephemeralSupported" @select="selectPane" @split="split" @resize="resize" @resizing="resizing = $event" @close="close" @maximize="maximize" @drop-pane="dropFromPointer" @add-pane="add" @create-session="(id,provider,ephemeral)=>emit('create-session',id,provider,ephemeral)" @reveal-session="emit('reveal-session', $event)">
        <template #pane="scope"><slot name="pane" :pane="scope.pane" /></template>
      </LayoutNode>
    </div>
    <div class="dock-canvas-hint">
      <span class="dock-arrange-hint">{{ t('Drag tabs to arrange · edges split · center stacks') }}</span>
      <div class="dock-layout-tools">
        <span v-if="collapsedCount && !maximized" class="dock-adaptive-label" :title="t('Panes expand automatically when more space is available')">{{ t('Responsive tabs') }}</span>
        <button v-if="maximized" type="button" class="dock-restore" @click="restoreView"><Icon name="minimize" :size="13" />{{ t('Restore layout') }}</button>
        <details ref="layoutMenu" class="dock-layout-menu" @keydown="layoutEscape">
          <summary :title="t('Layout options')"><Icon name="layout" :size="14" /><span>{{ t('Layout options') }}</span><Icon name="chevron" :size="11" /></summary>
          <div class="dock-layout-options">
            <div class="dock-menu-label">{{ t('Layout presets') }}</div>
            <button type="button" :title="t('Two equal columns; all tabs are preserved')" :aria-label="t('Layout 1:1')" @click="preset('1:1')"><Icon name="splitHorizontal" :size="15" /><span>{{ t('Two equal columns') }}</span><small>1:1</small></button>
            <button type="button" :title="t('Four panes in a grid; all tabs are preserved')" :aria-label="t('Layout 2 by 2')" @click="preset('2x2')"><Icon name="grid" :size="15" /><span>{{ t('Four-pane grid') }}</span><small>2×2</small></button>
            <button type="button" :title="t('Three columns with a larger center; all tabs are preserved')" :aria-label="t('Layout 1:2:1')" @click="preset('1:2:1')"><Icon name="layout" :size="15" /><span>{{ t('Wide center') }}</span><small>1:2:1</small></button>
            <div class="dock-menu-divider" />
            <button type="button" @click="splitSelected('horizontal')"><Icon name="splitHorizontal" :size="15" /><span>{{ t('Split side by side') }}</span></button>
            <button type="button" @click="splitSelected('vertical')"><Icon name="splitVertical" :size="15" /><span>{{ t('Split top and bottom') }}</span></button>
            <div class="dock-menu-note">{{ t('Resize · snap · {width} × {height} min', { width: minWidth, height: minHeight }) }}</div>
          </div>
        </details>
      </div>
    </div>
  </div>
</template>

<style scoped>
.dock-canvas { display:flex;flex-direction:column;flex:1;min-width:0;min-height:0;width:100%;height:100%;box-sizing:border-box; }
.dock-layout-tools { display:flex;align-items:center;gap:7px;margin-left:auto;flex-shrink:0; }.dock-adaptive-label { color:#789a90;font-size:9px; }.dock-restore { display:inline-flex;align-items:center;gap:5px;background:transparent;color:#198c7d;border:0;font:inherit;padding:4px 6px;border-radius:5px;cursor:pointer; }
.dock-layout-menu { position:relative; }.dock-layout-menu>summary { display:flex;align-items:center;gap:5px;min-height:25px;padding:2px 5px;border-radius:5px;list-style:none;color:#698b85;font-size:9px;cursor:pointer;box-sizing:border-box; }.dock-layout-menu>summary::-webkit-details-marker { display:none; }.dock-layout-menu>summary>svg:last-child { transform:rotate(-90deg); }.dock-layout-menu[open]>summary,.dock-layout-menu>summary:hover,.dock-restore:hover { background:#e7f4f0;color:#0c8376; }.dock-layout-menu>summary:focus-visible,.dock-layout-options button:focus-visible,.dock-restore:focus-visible { outline:2px solid #51b9b0;outline-offset:1px; }
.dock-layout-options { position:absolute;right:0;bottom:calc(100% + 5px);z-index:40;width:230px;max-width:calc(100vw - 28px);padding:6px;border:1px solid #dce7e3;border-radius:9px;background:#fff;box-shadow:0 8px 28px #203f4320; }.dock-layout-options>button { display:flex;align-items:center;gap:9px;width:100%;min-height:32px;border:0;border-radius:5px;padding:7px;background:transparent;font-family:inherit;font-size:11px;color:#617b84;text-align:left;cursor:pointer; }.dock-layout-options>button:hover { background:#ecf7f3;color:#0c8376; }.dock-layout-options>button>span { flex:1; }.dock-layout-options small { font-size:9px;color:#95a6ae; }.dock-menu-label { padding:4px 7px 6px;font-size:9px;font-weight:600;color:#9ba9b0; }.dock-menu-divider { height:1px;background:#ecf1ef;margin:5px 4px; }.dock-menu-note { padding:6px 7px 3px;font-size:9px;color:#9aaab1;line-height:15px; }
.dock-canvas-stage { flex:1;min-width:0;min-height:0;position:relative;overflow:hidden; }
.dock-canvas-hint { display:flex;align-items:center;justify-content:space-between;gap:8px;min-height:27px;color:#9caab3;font-size:9px;line-height:14px;padding:3px 1px 0;box-sizing:border-box; }.dock-arrange-hint { min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap; }
.dock-canvas-notice { display:flex;justify-content:space-between;background:#fff7e7;border:1px solid #e9d3a7;border-radius:6px;padding:7px 9px;color:#957534;font-size:11px;margin-bottom:6px; }.dock-canvas-notice button { border:0;background:transparent;cursor:pointer;color:inherit; }
.is-resizing { user-select:none; }
@media(max-width:700px) { .dock-adaptive-label { display:none; }.dock-canvas-hint { font-size:8px; } }
@media(pointer:coarse) { .dock-layout-menu>summary,.dock-restore { min-height:40px; }.dock-layout-options>button { min-height:44px; } }
</style>
