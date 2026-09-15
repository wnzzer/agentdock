<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import type { ComponentPublicInstance } from "vue";
import type { FileEntry } from "@agentdock/protocol";
import { errorMessage, formatBytes, request, workspacePath } from "./api";
import { flattenTree, isTreeChild, sortTreeEntries, treeAncestors, treeParent } from "./tree-model";
import type { TreeDirectory, TreeRow } from "./tree-model";
import { readTreeViewState, saveTreeViewState } from "./tree-state";
import Icon from "./Icon.vue";
import { filePane } from "./pane-context";
import { useI18n } from "../i18n";

const { t } = useI18n();
const props = defineProps<{ workspaceId: string; refreshToken?: number; selectedPath?: string }>();
const emit = defineEmits<{ open: [file: FileEntry]; close: [] }>();
const directories = ref(new Map<string, TreeDirectory>());
const expanded = ref(new Set<string>());
const query = ref("");
const selected = ref(props.selectedPath ?? "");
const focusedPath = ref("");
const revealFailure = ref("");
const error = ref("");
/**
 * Row context menu.
 *
 * Copying a path is the thing people came to the tree for most often and had
 * to do by hand; deleting was not possible here at all.
 */
const rowMenu = ref<{ entry: FileEntry; x: number; y: number } | null>(null);
const copied = ref("");
const deleting = ref(false);
const confirmDelete = ref(false);
function openRowMenu(event: MouseEvent, entry: FileEntry) {
  event.preventDefault();
  confirmDelete.value = false; copied.value = "";
  rowMenu.value = { entry, x: event.clientX, y: event.clientY };
}
function closeRowMenu() { rowMenu.value = null; confirmDelete.value = false; }
const rowMenuStyle = computed(() => rowMenu.value
  ? { left: Math.min(rowMenu.value.x, window.innerWidth - 210) + "px", top: Math.min(rowMenu.value.y, window.innerHeight - 190) + "px" }
  : {});
/** Clipboard access can be refused (insecure origin, denied permission), so a
 * failure says so rather than silently doing nothing. */
async function copyText(value: string, label: string) {
  try {
    await navigator.clipboard.writeText(value);
    copied.value = label;
    setTimeout(() => { if (copied.value === label) copied.value = ""; }, 1400);
  } catch { error.value = t("Could not copy to the clipboard in this browser."); closeRowMenu(); }
}
async function deleteEntry() {
  const entry = rowMenu.value?.entry; if (!entry) return;
  deleting.value = true;
  try {
    await request(workspacePath(props.workspaceId) + "/file" + "?path=" + encodeURIComponent(entry.path), { method: "DELETE", headers: { "X-AgentDock-Client": "web" } });
    closeRowMenu();
    await loadDirectory(treeParent(entry.path) ?? "", true);
  } catch (cause) { error.value = errorMessage(cause); }
  finally { deleting.value = false; }
}
const refreshing = ref(false);
const treeElement = ref<HTMLElement>();
const rowElements = new Map<string, HTMLElement>();
let generation = 0, revealRevision = 0;
let pending = new Map<string, Promise<boolean>>();
let restorePaths: string[] = [];

const rows = computed(() => flattenTree(directories.value, expanded.value, query.value));
const root = computed(() => directories.value.get(""));
const loading = computed(() => refreshing.value || [...directories.value.values()].some(directory => directory.loading));
const loadedCount = computed(() => flattenTree(directories.value, new Set(directories.value.keys())).length);
const tabPath = computed(() => rows.value.find(row => row.entry.path === focusedPath.value)?.entry.path ?? rows.value.find(row => row.entry.path === selected.value)?.entry.path ?? rows.value[0]?.entry.path);
const crumbs = computed(() => {
  if (!selected.value) return [];
  const parts = selected.value.split("/");
  return parts.map((name, index) => ({ name, path: parts.slice(0, index + 1).join("/") }));
});

function directory(path: string): TreeDirectory {
  if (!directories.value.has(path)) directories.value.set(path, { entries: [], loaded: false, loading: false, error: "" });
  return directories.value.get(path)!;
}

function loadDirectory(path: string, force = false): Promise<boolean> {
  if (pending.has(path)) return pending.get(path)!;
  const state = directory(path);
  if (state.loaded && !force) return Promise.resolve(true);
  const requestGeneration = generation;
  state.loading = true;
  state.error = "";
  const operation = request<FileEntry[]>(`${workspacePath(props.workspaceId)}/files?path=${encodeURIComponent(path)}`)
    .then(entries => {
      if (generation !== requestGeneration) return false;
      state.entries = sortTreeEntries(entries.filter(entry => isTreeChild(path, entry.path)));
      state.loaded = true;
      return true;
    })
    .catch(cause => {
      if (generation === requestGeneration) state.error = errorMessage(cause);
      return false;
    })
    .finally(() => {
      if (generation === requestGeneration) { state.loading = false; pending.delete(path); }
    });
  pending.set(path, operation);
  return operation;
}

function setRowRef(path: string, element: Element | ComponentPublicInstance | null) {
  if (element instanceof HTMLElement) rowElements.set(path, element);
  else rowElements.delete(path);
}

async function focusRow(path: string) {
  focusedPath.value = path;
  await nextTick();
  const element = rowElements.get(path);
  element?.scrollIntoView({ block: "nearest", inline: "nearest" });
  element?.focus({ preventScroll: true });
}

async function toggle(file: FileEntry) {
  if (file.kind !== "directory") return;
  const wasExpanded = rows.value.find(row => row.entry.path === file.path)?.expanded ?? expanded.value.has(file.path);
  clearFilterKeepingPath(file.path);
  if (wasExpanded) expanded.value.delete(file.path);
  else { expanded.value.add(file.path); await loadDirectory(file.path); }
}

function clearFilterKeepingPath(path: string) {
  if (!query.value.trim()) return;
  query.value = "";
  for (const ancestor of treeAncestors(path)) if (ancestor) expanded.value.add(ancestor);
}

function open(file: FileEntry) {
  selected.value = file.path;
  focusedPath.value = file.path;
  revealFailure.value = "";
  if (file.kind === "directory") void toggle(file);
  else emit("open", file);
}

async function navigate(event: KeyboardEvent, row: TreeRow) {
  if (event.altKey || event.ctrlKey || event.metaKey) return;
  if (!["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "Home", "End", "Enter", " "].includes(event.key)) return;
  event.preventDefault(); event.stopPropagation();
  const index = rows.value.findIndex(item => item.entry.path === row.entry.path);
  if (event.key === "Enter" || event.key === " ") { open(row.entry); return; }
  if (event.key === "ArrowDown") { const next = rows.value[index + 1]; if (next) await focusRow(next.entry.path); }
  else if (event.key === "ArrowUp") { const previous = rows.value[index - 1]; if (previous) await focusRow(previous.entry.path); }
  else if (event.key === "Home" || event.key === "End") {
    const next = event.key === "Home" ? rows.value[0] : rows.value.at(-1);
    if (next) await focusRow(next.entry.path);
  } else if (event.key === "ArrowRight" && row.entry.kind === "directory") {
    if (!row.expanded) await toggle(row.entry);
    else { const child = rows.value[index + 1]; if (child?.parent === row.entry.path) await focusRow(child.entry.path); }
  } else if (event.key === "ArrowLeft") {
    // Filter results temporarily reveal ancestors; clear the filter before explicitly collapsing.
    if (row.entry.kind === "directory" && row.expanded) { clearFilterKeepingPath(row.entry.path); expanded.value.delete(row.entry.path); }
    else if (row.parent) await focusRow(row.parent);
  }
}

async function refresh() {
  if (refreshing.value) return;
  const remembered = new Set([...directories.value.keys(), ...expanded.value, ...restorePaths]);
  restorePaths = [];
  // Revalidate collapsed ancestors too, so a remembered nested expansion remains meaningful.
  for (const path of [...remembered]) if (path) for (const parent of treeAncestors(path)) remembered.add(parent);
  const reloadPaths = [...remembered].filter(Boolean);
  generation++; revealRevision++;
  const refreshGeneration = generation;
  pending = new Map();
  directories.value = new Map();
  refreshing.value = true;
  revealFailure.value = "";
  try {
    if (!await loadDirectory("")) return;
    for (const path of reloadPaths.sort((a, b) => a.split("/").length - b.split("/").length)) {
      if (generation !== refreshGeneration) return;
      const entry = directories.value.get(treeParent(path))?.entries.find(file => file.path === path);
      if (entry?.kind === "directory") await loadDirectory(path);
      else expanded.value.delete(path);
    }
  } finally { if (generation === refreshGeneration) refreshing.value = false; }
}

/** Reveal uses only the target's ancestors and never scans unrelated directories. */
async function reveal(path: string): Promise<void> {
  const requestRevision = ++revealRevision, requestGeneration = generation;
  const current = () => requestRevision === revealRevision && requestGeneration === generation;
  query.value = ""; revealFailure.value = "";
  let ancestors: string[];
  try { ancestors = treeAncestors(path); }
  catch { revealFailure.value = path; return; }
  for (const ancestor of ancestors) {
    if (!current()) return;
    if (ancestor) {
      const parent = treeParent(ancestor);
      if (!directories.value.get(parent)?.entries.some(file => file.path === ancestor)) await loadDirectory(parent, true);
      if (!current()) return;
      if (directories.value.get(parent)?.entries.find(file => file.path === ancestor)?.kind !== "directory") { revealFailure.value = path; return; }
      expanded.value.add(ancestor);
    }
    if (!await loadDirectory(ancestor) || !current()) return;
  }
  const parent = treeParent(path);
  if (!directories.value.get(parent)?.entries.some(file => file.path === path)) await loadDirectory(parent, true);
  if (!current()) return;
  if (!directories.value.get(parent)?.entries.some(file => file.path === path)) { revealFailure.value = path; return; }
  selected.value = path;
  focusedPath.value = path;
  await nextTick();
  if (!current()) return;
  const element = rowElements.get(path);
  element?.scrollIntoView({ block: "nearest", inline: "nearest" });
  element?.focus({ preventScroll: true });
}

async function focusRoot() {
  query.value = "";
  await nextTick();
  if (rows.value[0]) await focusRow(rows.value[0].entry.path);
  else treeElement.value?.focus();
}

function drag(event: DragEvent, file: FileEntry) {
  if (file.kind !== "file") return;
  event.dataTransfer?.setData("application/agentdock-pane", JSON.stringify(filePane(props.workspaceId, file.path)));
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copy";
}

function rememberView(workspaceId: string) {
  saveTreeViewState(workspaceId, {
    expanded: expanded.value,
    loadedDirectories: new Set([...directories.value].filter(([, state]) => state.loaded).map(([path]) => path)),
    selectedPath: selected.value, focusedPath: focusedPath.value, query: query.value,
  });
}

watch(() => props.workspaceId, (workspaceId, previousId) => {
  if (previousId) rememberView(previousId);
  const remembered = readTreeViewState(workspaceId);
  generation++; revealRevision++; pending = new Map();
  directories.value = new Map(); expanded.value = remembered.expanded;
  selected.value = props.selectedPath ?? remembered.selectedPath; focusedPath.value = remembered.focusedPath;
  query.value = remembered.query; revealFailure.value = ""; refreshing.value = false;
  restorePaths = [...remembered.loadedDirectories];
  void refresh();
}, { immediate: true });
watch(() => props.refreshToken, () => void refresh());
watch(() => props.selectedPath, path => { selected.value = path ?? ""; });
onBeforeUnmount(() => { rememberView(props.workspaceId); generation++; revealRevision++; });
defineExpose({ reveal });
</script>
<template>
  <section class="file-explorer">
    <header class="section-header"><span><Icon name="folder" /><strong>{{ t('Explorer') }}</strong></span><span><button class="icon-button" :aria-label="t('Refresh files')" :title="t('Refresh files')" :disabled="loading" @click="refresh"><Icon name="refresh" :size="15" /></button><button class="icon-button explorer-close" :aria-label="t('Close explorer')" @click="emit('close')"><Icon name="close" :size="16" /></button></span></header>
    <nav class="file-breadcrumb" :aria-label="t('Directory path')"><button @click="focusRoot">{{ t('Root') }}</button><template v-for="crumb in crumbs" :key="crumb.path"><span>/</span><button :title="crumb.path" @click="reveal(crumb.path)">{{ crumb.name }}</button></template></nav>
    <div class="file-search"><Icon name="search" :size="14" /><input v-model="query" :aria-label="t('Filter loaded files')" :placeholder="t('Filter loaded files…')" /><button v-if="query" class="icon-button clear-filter" :aria-label="t('Clear file filter')" @click="query = ''"><Icon name="close" :size="12" /></button></div>
    <p v-if="query" class="tree-filter-note">{{ t('Only loaded folders are searched.') }}</p>
    <p v-if="revealFailure" class="inline-error" role="alert">{{ t('Could not locate {path} in the workspace.', { path: revealFailure }) }}</p>
    <p v-if="error" class="inline-error" role="alert">{{ error }}<button class="text-button" @click="error = ''">{{ t('Dismiss') }}</button></p>
    <div v-if="root?.error" class="inline-error tree-error" role="alert"><span>{{ root.error }}</span><button class="text-button" @click="loadDirectory('', true)">{{ t('Retry') }}</button></div>
    <div ref="treeElement" class="file-list file-tree" role="tree" :aria-label="t('Workspace files')" :aria-busy="refreshing || root?.loading" :tabindex="rows.length ? -1 : 0">
      <template v-for="row in rows" :key="row.entry.path">
        <button :ref="element => setRowRef(row.entry.path, element)" class="file-row tree-row" :class="{ 'is-selected': selected === row.entry.path, 'is-directory': row.entry.kind === 'directory' }" role="treeitem" :aria-level="row.depth + 1" :aria-posinset="row.position" :aria-setsize="row.siblings" :aria-expanded="row.entry.kind === 'directory' ? row.expanded : undefined" :aria-selected="selected === row.entry.path" :aria-label="row.entry.name" :aria-description="row.entry.path" :aria-busy="row.entry.kind === 'directory' ? directories.get(row.entry.path)?.loading : undefined" :tabindex="tabPath === row.entry.path ? 0 : -1" :style="{ '--tree-depth': row.depth }" :title="row.entry.path" :draggable="row.entry.kind === 'file'" @contextmenu="openRowMenu($event, row.entry)" @dragstart="drag($event, row.entry)" @click="open(row.entry)" @focus="focusedPath = row.entry.path" @keydown="navigate($event, row)">
          <i class="tree-disclosure" :class="{ expanded: row.expanded }"><Icon v-if="row.entry.kind === 'directory'" name="chevron" :size="11" /></i><Icon :name="row.entry.kind === 'directory' ? 'folder' : 'file'" :size="15" /><span>{{ row.entry.name }}</span><small v-if="row.entry.kind === 'symlink'" :title="t('Symbolic link')">↗</small><small v-else-if="row.entry.kind !== 'directory'">{{ formatBytes(row.entry.size) }}</small>
        </button>
        <div v-if="row.expanded && !query.trim()" role="none" class="tree-folder-state" :style="{ '--tree-depth': row.depth + 1 }">
          <p v-if="directories.get(row.entry.path)?.loading" role="status">{{ t('Loading files…') }}</p>
          <div v-else-if="directories.get(row.entry.path)?.error" class="tree-error" role="alert"><span>{{ directories.get(row.entry.path)?.error }}</span><button class="text-button" @click="loadDirectory(row.entry.path, true)">{{ t('Retry') }}</button></div>
          <p v-else-if="directories.get(row.entry.path)?.loaded && !directories.get(row.entry.path)?.entries.length">{{ t('This directory is empty.') }}</p>
        </div>
      </template>
      <div v-if="root?.loading" class="small-empty" role="status">{{ t('Loading files…') }}</div>
      <div v-else-if="root?.loaded && !rows.length" class="small-empty">{{ t(query.trim() ? 'No matching loaded files.' : 'This directory is empty.') }}</div>
    </div>
    <Teleport to="body"><div v-if="rowMenu" class="tree-menu-backdrop" @pointerdown="closeRowMenu" @contextmenu.prevent="closeRowMenu"><nav class="tree-menu" :style="rowMenuStyle" role="menu" :aria-label="t('File actions')" @pointerdown.stop @keydown.esc.stop.prevent="closeRowMenu"><button type="button" role="menuitem" @click="copyText(rowMenu.entry.path, 'path')">{{ t(copied === 'path' ? 'Copied' : 'Copy path') }}</button><button type="button" role="menuitem" @click="copyText(rowMenu.entry.name, 'name')">{{ t(copied === 'name' ? 'Copied' : 'Copy name') }}</button><button v-if="rowMenu.entry.kind === 'file'" type="button" role="menuitem" @click="closeRowMenu(); open(rowMenu!.entry)">{{ t('Open') }}</button><button type="button" role="menuitem" @click="closeRowMenu(); loadDirectory(rowMenu!.entry.kind === 'directory' ? rowMenu!.entry.path : treeParent(rowMenu!.entry.path) ?? '', true)">{{ t('Refresh files') }}</button><hr/><template v-if="confirmDelete"><p class="tree-menu-confirm">{{ t(rowMenu.entry.kind === 'directory' ? 'Delete {name} and everything inside it? This cannot be undone.' : 'Delete {name}? This cannot be undone.', { name: rowMenu.entry.name }) }}</p><button type="button" role="menuitem" class="tree-menu-danger" :disabled="deleting" :aria-busy="deleting" @click="deleteEntry">{{ t(deleting ? 'Deleting…' : 'Delete permanently') }}</button><button type="button" role="menuitem" @click="confirmDelete = false">{{ t('Cancel') }}</button></template><button v-else type="button" role="menuitem" class="tree-menu-danger" @click="confirmDelete = true">{{ t('Delete…') }}</button></nav></div></Teleport>
    <footer class="explorer-footer">{{ t('{count} loaded items · host filesystem', { count: loadedCount }) }}<span>{{ t('Click to open · drag into a pane') }}</span></footer>
  </section>
</template>
<style scoped>
/* Teleported so the tree's own scrolling and clipping cannot cut it off. */
.tree-menu-backdrop{position:fixed;inset:0;z-index:60}
.tree-menu{position:fixed;min-width:198px;padding:5px;background:var(--surface);border:1px solid var(--border);border-radius:11px;box-shadow:0 14px 38px #243b4c2b}
.tree-menu button{display:block;width:100%;min-height:32px;padding:7px 10px;border:0;border-radius:7px;background:none;text-align:left;font-size:12px;color:var(--ink-soft);white-space:nowrap;cursor:pointer}
.tree-menu button:hover:not(:disabled){background:var(--teal-soft);color:var(--teal)}
.tree-menu button:disabled{opacity:.5;cursor:not-allowed}
.tree-menu hr{border:0;border-top:1px solid var(--border);margin:4px 6px}
.tree-menu-danger{color:var(--danger-ink)}
.tree-menu-danger:hover:not(:disabled){background:#fff3f5;color:var(--danger)}
.tree-menu-confirm{padding:6px 10px;margin:0;font-size:11px;line-height:1.6;color:var(--ink-soft);white-space:normal;max-width:220px}
@media(pointer:coarse){.tree-menu button{min-height:44px}}
.file-breadcrumb { max-height: 66px; overflow: auto; }
.file-breadcrumb button { max-width: 100%; }
.file-search { gap: 5px; }
.file-search input { min-width: 0; }
.clear-filter { width: 17px; height: 17px; padding: 1px; }
.tree-filter-note { margin: -5px 14px 10px; font-size: 9px; line-height: 15px; color: #80949f; }
.file-tree { padding-top: 2px; scroll-padding: 6px; }
.tree-row { padding: 7px 7px 7px calc(3px + var(--tree-depth) * 15px); gap: 5px; position: relative; }
.tree-row > svg { flex-shrink: 0; color: #98a9b2; }
.tree-row.is-directory > svg { color: #82a79d; }
.tree-row > span { min-width: 38px; }
.tree-row.is-selected { background: #e8f4ef; box-shadow: inset 2px 0 #45a18c; }
.tree-row.is-selected > span { color: #296e60; }
.tree-row:focus-visible { outline: 1px solid #55a692; outline-offset: -2px; background: #f1f8f5; }
.tree-disclosure { display: flex; width: 11px; height: 15px; align-items: center; justify-content: center; flex: 0 0 11px; color: #9aaaae; }
.tree-disclosure > svg { transition: transform 120ms ease; }
.tree-disclosure.expanded > svg { transform: rotate(90deg); }
.tree-folder-state { padding-left: calc(25px + var(--tree-depth) * 15px); }
.tree-folder-state:empty { display: none; }
.tree-folder-state p { color: #9aabb3; font-size: 9px; line-height: 17px; padding: 3px 0 7px; }
.tree-error { display: flex; gap: 7px; align-items: baseline; font-size: 9px; line-height: 16px; padding: 4px 0 8px; color: #af6876; }
.tree-error > span { min-width: 0; overflow-wrap: anywhere; }
.tree-error > button { flex-shrink: 0; color: #728e86; font-size: 9px; }
.inline-error.tree-error { margin: 0 12px 8px; }
@media (prefers-reduced-motion: reduce) { .tree-disclosure > svg { transition: none; } }
</style>
