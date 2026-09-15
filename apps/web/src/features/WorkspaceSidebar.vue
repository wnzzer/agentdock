<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { ComponentPublicInstance } from "vue";
import type { ProviderKind, Session, Workspace } from "@agentdock/protocol";
import { providerLabel } from "./api";
import { emptyWorkspaceGroupPreferences, groupWorkspaces, loadWorkspaceGroupPreferences, saveWorkspaceGroupPreferences, workspaceGroupStorageKey } from "./workspace-groups";
import type { WorkspaceGroup } from "./workspace-groups";
import { filterSessionList, isEphemeralSession, isSessionArchived } from "./session-list";
import type { SessionArchiveFilter, SessionEphemeralFilter } from "./session-list";
import { useI18n } from "../i18n";
import Icon from "./Icon.vue";
import ProviderIcon from "./ProviderIcon.vue";
import { lastQuickProvider } from "./quick-session";
import SidebarSessionRow from "./SidebarSessionRow.vue";

const props = withDefaults(defineProps<{
  workspaces: Workspace[];
  sessions: Session[];
  selectedWorkspaceId?: string;
  selectedSessionId?: string;
  historySupported?: boolean;
  archiveSupported?: boolean;
  ephemeralSupported?: boolean;
  archiveBusyIds?: string[];
  keepBusyIds?: string[];
  sessionsLoading?: boolean;
  storageKey?: string;
}>(), { historySupported: false, archiveSupported: false, ephemeralSupported: false, archiveBusyIds: () => [], keepBusyIds: () => [], sessionsLoading: false });
const emit = defineEmits<{
  selectWorkspace: [id: string];
  openSession: [session: Session];
  locateSession: [session: Session];
  sessionEnvironment: [id: string];
  renameSession: [session: Session, title: string];
  openFiles: [id: string];
  openChanges: [id: string];
  newSession: [id: string, provider?: ProviderKind];
  quickSession: [id: string, provider: ProviderKind, ephemeral: boolean];
  loadHistory: [id: string];
  addWorkspace: [];
  allSessions: [];
  canvas: [];
  archiveSession: [session: Session, archived: boolean];
  keepSession: [session: Session];
  refreshSessions: [];
}>();
const { t } = useI18n();
const query = ref("");
const preferences = ref(emptyWorkspaceGroupPreferences());
const searchExpanded = ref<Record<string, boolean>>({});
const openMenuId = ref<string>();
const menuButtons = new Map<string, HTMLElement>();
const sidebarElement = ref<HTMLElement>();
const allSessionsExpanded = ref(false);
const sessionFiltersExpanded = ref(false);
const sessionQuery = ref("");
const sessionWorkspace = ref("");
const sessionProvider = ref<ProviderKind | "">("");
const sessionStatus = ref<Session["status"] | "">("");
const sessionArchive = ref<SessionArchiveFilter>("current");
const sessionEphemeral = ref<SessionEphemeralFilter>("all");
const sessionStatuses: Session["status"][] = ["starting", "running", "waiting", "stopped", "failed"];
const archiveBusySet = computed(() => new Set(props.archiveBusyIds));
const keepBusySet = computed(() => new Set(props.keepBusyIds));
const sessionCount = computed(() => new Set(props.sessions.map(session => session.id)).size);
const filteredSessions = computed(() => filterSessionList(props.sessions, props.workspaces, {
  query: sessionQuery.value, workspaceId: sessionWorkspace.value, provider: sessionProvider.value,
  status: sessionStatus.value, archive: sessionArchive.value, ephemeral: sessionEphemeral.value,
  statusLabels: Object.fromEntries(sessionStatuses.map(status => [status, t(status)])),
}));
const hasSessionFilters = computed(() => Boolean(sessionWorkspace.value || sessionProvider.value || sessionStatus.value || sessionArchive.value !== "current" || sessionEphemeral.value !== "all"));
const persistenceKey = computed(() => workspaceGroupStorageKey(props.storageKey ?? (typeof window === "undefined" ? "default" : window.location.origin)));
const groups = computed(() => groupWorkspaces(props.workspaces, props.sessions, { query: query.value, selectedWorkspaceId: props.selectedWorkspaceId, preferences: preferences.value, searchExpanded: searchExpanded.value }));
const canvasWorkspaceId = computed(() => props.workspaces.some(workspace => workspace.id === props.selectedWorkspaceId) ? props.selectedWorkspaceId : props.workspaces[0]?.id);

function storage(): Storage | undefined {
  try { return typeof localStorage === "undefined" ? undefined : localStorage; } catch { return undefined; }
}
function persist() { saveWorkspaceGroupPreferences(persistenceKey.value, preferences.value, storage()); }
watch(persistenceKey, key => { preferences.value = loadWorkspaceGroupPreferences(key, storage()); query.value = ""; searchExpanded.value = {}; openMenuId.value = undefined; allSessionsExpanded.value = false; clearSessionFilters(); }, { immediate: true });
watch(query, () => { searchExpanded.value = {}; openMenuId.value = undefined; });
watch(() => props.workspaces, workspaces => { if (openMenuId.value && !workspaces.some(workspace => workspace.id === openMenuId.value)) openMenuId.value = undefined; });

function setExpanded(id: string, value: boolean) {
  if (query.value.trim()) searchExpanded.value = { ...searchExpanded.value, [id]: value };
  else { preferences.value.expanded = { ...preferences.value.expanded, [id]: value }; persist(); }
}
function selectWorkspace(group: WorkspaceGroup) { setExpanded(group.workspace.id, true); openMenuId.value = undefined; emit("selectWorkspace", group.workspace.id); }
function togglePin(group: WorkspaceGroup) {
  const id = group.workspace.id;
  preferences.value.pinned = group.pinned ? preferences.value.pinned.filter(value => value !== id) : [id, ...preferences.value.pinned.filter(value => value !== id)];
  persist(); void closeMenu(true);
}
function recordMenuButton(id: string, element: Element | ComponentPublicInstance | null) {
  if (element instanceof HTMLElement) menuButtons.set(id, element); else menuButtons.delete(id);
}
async function closeMenu(restoreFocus = false) {
  const id = openMenuId.value; openMenuId.value = undefined;
  if (restoreFocus && id) { await nextTick(); menuButtons.get(id)?.focus(); }
}
function terminal(id: string) { openMenuId.value = undefined; emit("newSession", id, "terminal"); }
/** Quick create never opens the dialog here; the owner decides if a choice is unavoidable. */
function quick(id: string, provider: ProviderKind, ephemeral = false) { openMenuId.value = undefined; emit("quickSession", id, provider, ephemeral); }
const quickProvider = computed(() => lastQuickProvider());
const quickLabel = computed(() => quickProvider.value === "terminal" ? t("Terminal") : providerLabel(quickProvider.value));
function history(id: string) { if (props.historySupported) { openMenuId.value = undefined; emit("loadHistory", id); } }
function escapeMenu(event: KeyboardEvent) { if (event.key === "Escape" && openMenuId.value) { event.preventDefault(); event.stopPropagation(); void closeMenu(true); } }
/** A right-click opens this menu without moving focus, so focus alone cannot
 * dismiss it; a pointer landing anywhere outside does. */
function closeMenuOutside(event: PointerEvent) {
  const id = openMenuId.value; if (!id) return;
  const target = event.target as Node | null;
  if (!target) return;
  const panel = document.getElementById(`workspace-menu-${id}`);
  if (panel?.contains(target) || menuButtons.get(id)?.contains(target)) return;
  openMenuId.value = undefined;
}
onMounted(() => document.addEventListener("pointerdown", closeMenuOutside));
onBeforeUnmount(() => document.removeEventListener("pointerdown", closeMenuOutside));
function navigateWorkspace(event: KeyboardEvent, group: WorkspaceGroup) {
  if (event.altKey || event.ctrlKey || event.metaKey) return;
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); event.stopPropagation(); setExpanded(group.workspace.id, event.key === "ArrowRight"); }
}
function toggleAllSessions() { allSessionsExpanded.value = !allSessionsExpanded.value; openMenuId.value = undefined; }
function clearSessionFilters() {
  sessionQuery.value = ""; sessionWorkspace.value = ""; sessionProvider.value = ""; sessionStatus.value = ""; sessionArchive.value = "current"; sessionEphemeral.value = "all";
}
function archiveSession(session: Session, archived: boolean) {
  if (props.archiveSupported && !archiveBusySet.value.has(session.id)) emit("archiveSession", session, archived);
}
function renameSession(session: Session, title: string) { emit("renameSession", session, title); }
function keepSession(session: Session) { if (!keepBusySet.value.has(session.id)) emit("keepSession", session); }

/** Reveal the actual bound session without changing the canvas selection or launching a client. */
async function revealSession(id: string): Promise<boolean> {
  const session = props.sessions.find(item => item.id === id);
  if (!session) return false;
  query.value = ""; searchExpanded.value = {}; openMenuId.value = undefined;
  preferences.value.expanded = { ...preferences.value.expanded, [session.workspace_id]: true }; persist();
  // Temporary windows are deliberately absent from the workspace groups, so the
  // library is where revealing one has to land.
  const needsLibrary = isSessionArchived(session) || isEphemeralSession(session) || !props.workspaces.some(workspace => workspace.id === session.workspace_id);
  if (needsLibrary) {
    clearSessionFilters(); sessionArchive.value = isSessionArchived(session) ? "archived" : "current"; allSessionsExpanded.value = true;
  }
  await nextTick();
  const container = sidebarElement.value?.querySelector(needsLibrary ? ".all-sessions-list" : ".workspace-groups");
  const row = Array.from(container?.querySelectorAll<HTMLElement>("[data-sidebar-session-id]") ?? []).find(element => element.dataset.sidebarSessionId === id);
  row?.scrollIntoView?.({ block: "nearest", inline: "nearest", behavior: "auto" });
  row?.querySelector<HTMLButtonElement>(".workspace-session")?.focus({ preventScroll: true });
  return true;
}
defineExpose({ revealSession });
</script>

<template>
  <div ref="sidebarElement" class="workspace-sidebar" @keydown="escapeMenu">
    <nav class="workspace-global-nav" :aria-label="t('Workspace navigation')">
      <button class="nav-item current" :disabled="!canvasWorkspaceId" @click="emit('canvas')"><Icon name="grid" :size="16" /><span>{{ t('Workspace canvas') }}</span></button>
      <button class="nav-item all-sessions-toggle" :class="{ expanded: allSessionsExpanded }" :aria-expanded="allSessionsExpanded" aria-controls="sidebar-session-library" @click="toggleAllSessions"><Icon name="clock" :size="16" /><span>{{ t('All sessions') }}</span><span v-if="sessionCount" class="count-badge">{{ sessionCount }}</span><Icon class="all-sessions-chevron" name="chevron" :size="11" /></button>
    </nav>
    <section v-if="allSessionsExpanded" id="sidebar-session-library" class="all-sessions-panel" :aria-label="t('Session library')">
      <div class="session-library-toolbar">
        <div class="session-library-search"><Icon name="search" :size="13" /><input v-model="sessionQuery" :aria-label="t('Search all sessions')" :placeholder="t('Find a session…')" /><button v-if="sessionQuery" class="icon-button" :aria-label="t('Clear navigation search')" @click="sessionQuery = ''"><Icon name="close" :size="12" /></button></div>
        <button class="icon-button session-filter-button" :class="{ selected: sessionFiltersExpanded || hasSessionFilters }" :aria-label="t('Session filters')" :title="t('Session filters')" :aria-expanded="sessionFiltersExpanded" aria-controls="sidebar-session-filters" @click="sessionFiltersExpanded = !sessionFiltersExpanded"><Icon name="settings" :size="14" /></button>
        <button class="icon-button session-refresh-button" :disabled="sessionsLoading" :aria-label="t('Refresh sessions')" :title="t('Refresh sessions')" @click="emit('refreshSessions')"><Icon name="refresh" :size="13" /></button>
      </div>
      <div v-if="sessionFiltersExpanded" id="sidebar-session-filters" class="session-library-filters">
        <select v-model="sessionWorkspace" :aria-label="t('Filter sessions by workspace')"><option value="">{{ t('All workspaces') }}</option><option v-for="workspace in workspaces" :key="workspace.id" :value="workspace.id">{{ workspace.name }}</option></select>
        <select v-model="sessionProvider" :aria-label="t('Filter sessions by provider')"><option value="">{{ t('All providers') }}</option><option v-for="provider in (['claude_code', 'codex', 'terminal'] as const)" :key="provider" :value="provider">{{ provider === 'terminal' ? t('Terminal') : providerLabel(provider) }}</option></select>
        <select v-model="sessionStatus" :aria-label="t('Filter sessions by status')"><option value="">{{ t('All statuses') }}</option><option v-for="status in sessionStatuses" :key="status" :value="status">{{ t(status) }}</option></select>
        <select v-model="sessionArchive" :aria-label="t('Filter sessions by archive state')"><option value="current">{{ t('Current sessions') }}</option><option value="archived">{{ t('Archived sessions') }}</option><option value="all">{{ t('All sessions') }}</option></select>
        <select v-model="sessionEphemeral" :aria-label="t('Filter temporary windows')"><option value="all">{{ t('Permanent and temporary') }}</option><option value="only">{{ t('Only temporary windows') }}</option><option value="hidden">{{ t('Hide temporary windows') }}</option></select>
      </div>
      <div class="session-library-summary"><span>{{ t(sessionArchive === 'archived' ? 'Archived sessions' : sessionArchive === 'all' ? 'All sessions' : 'Current sessions') }} <small>{{ filteredSessions.length }}</small></span><button v-if="hasSessionFilters || sessionQuery" class="text-button" @click="clearSessionFilters">{{ t('Clear session filters') }}</button></div>
      <ul class="all-sessions-list" :aria-label="t('All sessions')" :aria-busy="sessionsLoading">
        <SidebarSessionRow v-for="entry in filteredSessions" :key="entry.session.id" :session="entry.session" :workspace-name="entry.workspace?.name ?? entry.session.workspace_id" :selected="selectedSessionId === entry.session.id" :archive-supported="archiveSupported" :archive-busy="archiveBusySet.has(entry.session.id)" :keep-busy="keepBusySet.has(entry.session.id)" @open="emit('openSession', $event)" @locate="emit('locateSession', $event)" @rename="renameSession" @environment="emit('sessionEnvironment', $event)" @archive="archiveSession" @keep="keepSession" />
        <li v-if="!filteredSessions.length" class="session-library-empty" role="status">{{ t(sessionsLoading ? 'Loading sessions…' : sessionArchive === 'archived' && !sessionQuery && !sessionWorkspace && !sessionProvider && !sessionStatus ? 'No archived sessions.' : 'No matching sessions.') }}</li>
      </ul>
    </section>
    <header class="workspace-group-label"><span>{{ t('Workspaces') }}</span><small>{{ workspaces.length }}</small><button class="icon-button" :aria-label="t('Add workspace')" :title="t('Add workspace')" @click="emit('addWorkspace')"><Icon name="plus" :size="15" /></button></header>
    <div v-if="workspaces.length" class="workspace-group-search"><Icon name="search" :size="13" /><input v-model="query" :aria-label="t('Search workspaces and sessions')" :placeholder="t('Find a workspace or session…')" /><button v-if="query" class="icon-button" :aria-label="t('Clear navigation search')" @click="query = ''"><Icon name="close" :size="12" /></button></div>
    <nav class="workspace-groups" :aria-label="t('Workspaces and sessions')">
      <section v-for="group in groups" :key="group.workspace.id" class="workspace-group" :class="{ selected: selectedWorkspaceId === group.workspace.id }" :aria-label="group.workspace.name">
        <div class="workspace-group-header" @contextmenu.prevent="openMenuId = group.workspace.id">
          <button class="icon-button workspace-disclosure" :class="{ expanded: group.expanded }" :aria-expanded="group.expanded" :aria-label="t(group.expanded ? 'Collapse {workspace} sessions' : 'Expand {workspace} sessions', { workspace: group.workspace.name })" :aria-controls="`workspace-sessions-${group.workspace.id}`" @click="setExpanded(group.workspace.id, !group.expanded)"><Icon name="chevron" :size="11" /></button>
          <button class="workspace-group-name" :title="group.workspace.root_path" :aria-expanded="group.expanded" :aria-controls="`workspace-sessions-${group.workspace.id}`" @click="selectWorkspace(group)" @keydown="navigateWorkspace($event, group)"><strong>{{ group.workspace.name }}</strong><svg v-if="group.pinned" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" :aria-label="t('Pinned workspace')"><path d="m16 3 5 5-4 1-4 5v4l-3-3-6 6 6-6-3-3h4l5-5z" /></svg></button>
          <span v-if="group.activeCount" class="workspace-active-count" :title="t('{count} active sessions', { count: group.activeCount })" :aria-label="t('{count} active sessions', { count: group.activeCount })"><i class="state-dot running" />{{ group.activeCount }}</span>
          <button :ref="element => recordMenuButton(group.workspace.id, element)" class="icon-button workspace-more-button" :class="{ selected: openMenuId === group.workspace.id }" :aria-label="t('Workspace actions for {workspace}', { workspace: group.workspace.name })" :aria-expanded="openMenuId === group.workspace.id" :aria-controls="`workspace-menu-${group.workspace.id}`" @click="openMenuId = openMenuId === group.workspace.id ? undefined : group.workspace.id"><Icon name="more" :size="15" /></button>
        </div>
        <div class="workspace-quick-actions">
          <button :title="t('Open files in {workspace}', { workspace: group.workspace.name })" :aria-label="t('Open files in {workspace}', { workspace: group.workspace.name })" @click="emit('openFiles', group.workspace.id)"><Icon name="folder" :size="12" /><span>{{ t('Files') }}</span></button>
          <button class="workspace-git-action" :title="t('Open Git changes in {workspace}', { workspace: group.workspace.name })" :aria-label="t('Open Git changes in {workspace}', { workspace: group.workspace.name })" @click="emit('openChanges', group.workspace.id)"><Icon name="git" :size="12" /><span>{{ t('Changes') }}</span></button>
          <button class="workspace-new-action" :title="t('New {provider} session in {workspace}', { provider: quickLabel, workspace: group.workspace.name })" :aria-label="t('New {provider} session in {workspace}', { provider: quickLabel, workspace: group.workspace.name })" @click="quick(group.workspace.id, quickProvider)"><Icon name="plus" :size="13" /></button>
        </div>
        <div v-if="openMenuId === group.workspace.id" :id="`workspace-menu-${group.workspace.id}`" class="workspace-more-panel">
          <button @click="quick(group.workspace.id, 'claude_code')"><ProviderIcon provider="claude_code" :size="13" />{{ t('New Claude Code session') }}</button>
          <button @click="quick(group.workspace.id, 'codex')"><ProviderIcon provider="codex" :size="13" />{{ t('New Codex session') }}</button>
          <button @click="quick(group.workspace.id, 'terminal')"><Icon name="terminal" :size="13" />{{ t('New terminal') }}</button>
          <button v-if="ephemeralSupported" class="workspace-scratch-action" :title="t('Discarded when its window is closed, and not kept in the session list.')" @click="quick(group.workspace.id, quickProvider, true)"><Icon name="clock" :size="13" />{{ t('New temporary window') }}</button>
          <button @click="openMenuId = undefined; emit('newSession', group.workspace.id)"><Icon name="settings" :size="13" />{{ t('New session… (more options)') }}</button>
          <span :title="historySupported ? undefined : t('Upgrade the backend to load native session history.')"><button :disabled="!historySupported" @click="history(group.workspace.id)"><Icon name="clock" :size="13" />{{ t('Load existing session') }}</button></span>
          <p v-if="!historySupported">{{ t('Upgrade the backend to load native session history.') }}</p>
          <button @click="togglePin(group)"><Icon name="check" :size="13" />{{ t(group.pinned ? 'Unpin workspace' : 'Pin workspace') }}</button>
        </div>
        <ul v-show="group.expanded" :id="`workspace-sessions-${group.workspace.id}`" class="workspace-session-list" :aria-label="t('Sessions in {workspace}', { workspace: group.workspace.name })">
        <SidebarSessionRow v-for="session in group.sessions" :key="session.id" :session="session" :selected="selectedSessionId === session.id" :archive-supported="archiveSupported" :archive-busy="archiveBusySet.has(session.id)" :keep-busy="keepBusySet.has(session.id)" @open="emit('openSession', $event)" @locate="emit('locateSession', $event)" @rename="renameSession" @environment="emit('sessionEnvironment', $event)" @archive="archiveSession" @keep="keepSession" />
          <li v-if="!group.sessions.length" class="workspace-group-empty"><p>{{ t('No sessions in this workspace.') }}</p><button class="text-button" @click="emit('newSession', group.workspace.id)"><Icon name="plus" :size="12" />{{ t('New session') }}</button></li>
        </ul>
      </section>
      <div v-if="workspaces.length && !groups.length" class="workspace-nav-empty"><Icon name="search" :size="22" /><p>{{ t('No matching workspaces or sessions.') }}</p><button class="text-button" @click="query = ''">{{ t('Clear navigation search') }}</button></div>
      <div v-else-if="!workspaces.length" class="workspace-nav-empty"><Icon name="folder" :size="24" /><p>{{ t('Add your first workspace') }}</p><button class="secondary-button" @click="emit('addWorkspace')"><Icon name="plus" :size="14" />{{ t('Add workspace') }}</button></div>
    </nav>
    <footer class="workspace-sidebar-footer"><Icon name="info" :size="12" /><span>{{ t('Drag sessions into the canvas to arrange them.') }}</span></footer>
  </div>
</template>

<style scoped>
.workspace-sidebar{display:flex;flex-direction:column;min-width:0;min-height:0;flex:1;overflow:hidden}.workspace-global-nav{padding-bottom:10px;border-bottom:1px solid #eaf0f2}.workspace-global-nav .nav-item{min-height:32px;padding:7px 8px;font-size:10px;gap:8px}.workspace-global-nav .count-badge{font-size:8px;min-width:17px;padding:1px 4px}.workspace-group-label{display:flex;align-items:center;gap:7px;padding:10px 5px 3px;font-size:10px;color:#8a9ca6;flex-shrink:0}.workspace-group-label>span{font-weight:600}.workspace-group-label>small{font-size:8px;color:#b0bac1}.workspace-group-label>.icon-button{margin-left:auto;width:24px;height:24px}.workspace-group-search{display:flex;align-items:center;gap:5px;padding:6px 7px;margin:3px 1px 9px;border:1px solid #e4ebee;border-radius:6px;background:#fff;color:#99aab3;flex-shrink:0}.workspace-group-search>input{width:100%;min-width:0;padding:0;border:0;background:none;font-size:9px;line-height:17px;color:#5e7883}.workspace-group-search>input::placeholder{color:#a2b0b8}.workspace-group-search>.icon-button{width:17px;height:17px;padding:2px}.workspace-groups{flex:1;min-height:0;overflow:auto;padding:0 1px 10px;scrollbar-width:thin}.workspace-group{border:1px solid transparent;border-radius:7px;margin:0 0 7px;min-width:0;padding:2px 2px 1px}.workspace-group.selected{background:#f7fbf9;border-color:#e0ece6}.workspace-group-header{display:flex;align-items:center;gap:3px;min-width:0;min-height:29px}.workspace-disclosure{width:17px;height:24px;padding:2px;color:#8da4ab}.workspace-disclosure>svg{transition:transform 120ms ease}.workspace-disclosure.expanded>svg{transform:rotate(90deg)}.workspace-group-name{display:flex;align-items:center;gap:5px;flex:1;min-width:0;padding:5px 0;border:0;background:none;text-align:left;color:#5c7580}.workspace-group-name>strong{font-size:10px;font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.workspace-group-name>svg{flex-shrink:0;color:#98a590}.workspace-group.selected .workspace-group-name{color:#317b6a}.workspace-group-name:hover{color:#14826f}.workspace-active-count{display:flex;align-items:center;gap:3px;font-size:8px;padding:2px 4px;border-radius:5px;background:#e7f3ec;color:#508f7d}.workspace-active-count>.state-dot{width:4px;height:4px}.workspace-more-button{width:21px;height:23px;padding:2px;font-size:17px;line-height:18px}.workspace-quick-actions{display:flex;align-items:center;gap:4px;padding:0 2px 5px 21px}.workspace-quick-actions>button{display:inline-flex;align-items:center;justify-content:center;gap:4px;border:0;border-radius:4px;background:none;font-size:8px;line-height:17px;color:#92a1a7;min-width:0;padding:2px 4px}.workspace-quick-actions>button:hover{background:#e9f3ee;color:#327d6d}.workspace-quick-actions>button>span{white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.workspace-quick-actions>.workspace-git-action{color:#a9946c}.workspace-quick-actions>.workspace-new-action{margin-left:auto;flex-shrink:0;width:22px}.workspace-more-panel{margin:2px 4px 8px 19px;padding:5px;border:1px solid #dfe9e5;border-radius:6px;background:#fff;box-shadow:0 3px 7px #203f4310}.workspace-more-panel button{display:flex;align-items:center;gap:6px;width:100%;border:0;background:none;border-radius:4px;padding:6px 5px;text-align:left;font-size:9px;line-height:15px;color:#6a828b}.workspace-more-panel button:hover:not(:disabled){background:#edf6f2;color:#247d69}.workspace-more-panel p{font-size:8px;line-height:14px;color:#a18e67;padding:1px 6px 6px}.workspace-session-list{list-style:none;margin:0 1px 3px 10px;padding:0 0 0 7px;border-left:1px solid #e5ecee}.workspace-group-empty{padding:8px 3px 9px;font-size:9px;color:#a5b2b9;line-height:16px}.workspace-group-empty>.text-button{font-size:8px;margin-top:3px}.workspace-nav-empty{padding:25px 8px;text-align:center;color:#a4b4ba}.workspace-nav-empty p{font-size:10px;line-height:18px;margin:10px 0}.workspace-nav-empty .secondary-button{font-size:9px;padding:5px 8px}.workspace-sidebar-footer{display:flex;align-items:center;gap:6px;flex-shrink:0;padding:10px 4px 12px;border-top:1px solid #edf1f3;color:#a1b1b7;font-size:8px;line-height:14px}.workspace-sidebar-footer>svg{color:#adbac0}
.workspace-global-nav{flex-shrink:0}.workspace-global-nav .all-sessions-toggle>.count-badge{margin-left:auto}.all-sessions-chevron{margin-left:auto;color:#8973b4;transition:transform 120ms ease}.all-sessions-toggle>.count-badge+.all-sessions-chevron{margin-left:0}.all-sessions-toggle.expanded>.all-sessions-chevron{transform:rotate(90deg)}.all-sessions-toggle.expanded{background:#f1edf8;color:#7c659f}.workspace-global-nav .nav-item>svg:first-child{color:#8973b4}.workspace-global-nav .nav-item.current>svg:first-child{color:#39856f}
.all-sessions-panel{display:flex;flex-direction:column;flex-shrink:1;min-height:100px;max-height:44vh;padding:8px 2px 7px;border-bottom:1px solid #e4ebe8}.session-library-toolbar{display:flex;align-items:center;gap:3px;flex-shrink:0}.session-library-search{display:flex;align-items:center;flex:1;min-width:0;gap:5px;padding:5px 6px;border:1px solid #e4ebee;border-radius:6px;background:#fff;color:#8f9db7}.session-library-search>input{width:100%;min-width:0;border:0;background:none;padding:0;font-size:9px;line-height:18px;color:#5e7883}.session-library-search>input::placeholder{color:#9dabb6}.session-library-search:focus-within{border-color:#ad9cc6;box-shadow:0 0 0 2px #eee8f8}.session-library-search>input:focus{outline:0}.session-library-search>.icon-button{width:17px;height:17px;padding:2px}.session-library-toolbar>.icon-button{flex:none;width:25px;height:28px;padding:5px}.session-filter-button{color:#8973b4}.session-filter-button.selected{background:#efeaf8}.session-refresh-button{color:#508f7d}.session-library-filters{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:5px;margin:7px 0 0;flex-shrink:0}.session-library-filters>select{min-width:0;width:100%;height:27px;padding:3px 4px;border:1px solid #dfe8e8;border-radius:5px;background:#fff;color:#667e89;font-size:9px}.session-library-summary{display:flex;align-items:center;justify-content:space-between;gap:5px;padding:6px 2px 3px;flex-shrink:0;min-width:0;font-size:8px;line-height:16px;color:#8a9aa7}.session-library-summary>span{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.session-library-summary small{margin-left:3px;font-size:8px;color:#a1afb9}.session-library-summary>.text-button{flex:none;font-size:8px;line-height:16px}.all-sessions-list{margin:0;padding:0;list-style:none;min-height:24px;overflow:auto;overscroll-behavior:contain;scrollbar-width:thin}.session-library-empty{padding:12px 4px;text-align:center;color:#95a5ad;font-size:9px;line-height:17px}.workspace-quick-actions>button:first-child>svg{color:#ad9467}.workspace-quick-actions>.workspace-git-action>svg{color:#c08d66}.workspace-quick-actions>.workspace-new-action>svg{color:#39856f}.workspace-more-panel button>svg{color:#8973b4}.workspace-group-label>.icon-button>svg{color:#39856f}.workspace-group-search>svg{color:#829cc0}
@media(pointer:coarse){.workspace-global-nav .nav-item{min-height:44px}.session-library-toolbar>.icon-button{width:36px;height:44px}.session-library-search{min-height:44px}.session-library-search>input{font-size:16px}.session-library-filters>select{height:44px;font-size:16px}.session-library-summary{font-size:11px}.session-library-summary>.text-button{min-height:32px;font-size:10px}.workspace-group-search>input{font-size:16px}.workspace-group-header{min-height:44px}.workspace-disclosure,.workspace-more-button{width:32px;height:44px}.workspace-quick-actions>button{min-height:36px;font-size:10px}.workspace-quick-actions>.workspace-new-action{width:36px}.workspace-more-panel button{min-height:44px;font-size:12px}}
@media(prefers-reduced-motion:reduce){.workspace-disclosure>svg,.all-sessions-chevron{transition:none}}
</style>
