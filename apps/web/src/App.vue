<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { shellHeight } from "./features/viewport-height";
import type { EndpointProfile, FileEntry, GitStatus, LayoutDocument, LayoutNode, PaneNode, ProviderKind, Session, Workspace } from "@agentdock/protocol";
import Canvas from "./components/Canvas.vue";
import { createDefaultLayoutDocument, flattenPanes, validateLayout } from "./layout/layout-engine";
import WorkspaceSidebar from "./features/WorkspaceSidebar.vue";
import WorkspacePane from "./features/WorkspacePane.vue";
import FileExplorer from "./features/FileExplorer.vue";
import LoadHistoryDialog from "./features/LoadHistoryDialog.vue";
import WorkspaceDialog from "./features/WorkspaceDialog.vue";
import SettingsDialog from "./features/SettingsDialog.vue";
import HostUsage from "./features/HostUsage.vue";
import ImageLightbox from "./features/ImageLightbox.vue";
import CreateSessionDialog from "./features/CreateSessionDialog.vue";
import SessionEnvironmentDialog from "./features/SessionEnvironmentDialog.vue";
import SessionRenameDialog from "./features/SessionRenameDialog.vue";
import AuthDialog from "./features/AuthDialog.vue";
import Icon from "./features/Icon.vue";
import { useI18n } from "./i18n";
import { ApiConnectionError, ApiError, errorMessage, json, request, workspacePath } from "./features/api";
import { paneString, paneWorkspace, paneSession, filePane, changesPane, sessionPane, findSessionPaneId, renameSessionPanes, scopeLegacyLayout, acceptsScopedPane, ephemeralSessionIds, withoutEphemeralPanes } from "./features/pane-context";
import { isEphemeralSession } from "./features/session-list";
import { lastQuickProvider, planQuickSession, rememberQuickProvider } from "./features/quick-session";
import { backendCapabilities, setBackendCapabilities, type BackendHealth } from "./features/backend-capabilities";
import { browserStorage, canvasStorageKey, readCanvasCache, writeCanvasCache, sameCanvasLayout } from "./features/canvas-cache";
import { readPanelVisibility, sidebarIsRail, writePanelVisibility } from "./features/panel-state";
import { hasDirtyDrafts } from "./features/file-drafts";
import { hasGitDrafts } from "./features/git-view-state";
import { sessionConnections } from "./features/session-connection";
import "@xterm/xterm/css/xterm.css";
import "./styles.css";

interface SharedCanvas { layout: LayoutDocument | null; revision: number }
const { t, locale, setLocale } = useI18n();
const workspaces = ref<Workspace[]>([]), sessions = ref<Session[]>([]), profiles = ref<EndpointProfile[]>([]);
const selectedWorkspaceId = ref(""), selectedPaneId = ref<string>();
const selectedWorkspace = computed(() => workspaces.value.find(workspace => workspace.id === selectedWorkspaceId.value));
const layout = ref<LayoutDocument>(createDefaultLayoutDocument()), canvasReady = ref(false);
const canvas = ref<InstanceType<typeof Canvas>>(), explorer = ref<InstanceType<typeof FileExplorer>>();
const workspaceSidebar = ref<InstanceType<typeof WorkspaceSidebar>>();
const focusedPane = computed(() => flattenPanes(layout.value.root).find(pane => pane.id === selectedPaneId.value));
const focusedWorkspace = computed(() => focusedPane.value ? paneWorkspace(focusedPane.value, workspaces.value, sessions.value) : undefined);
const contextWorkspace = computed(() => focusedWorkspace.value ?? selectedWorkspace.value);
const selectedSessionId = computed(() => focusedPane.value ? paneString(focusedPane.value, "session_id") : undefined);
const activeSessions = computed(() => sessions.value.filter(session => ["running", "waiting", "starting"].includes(session.status)));
const sessionProviders = computed(() => Object.fromEntries(sessions.value.map(session => [session.id, session.provider])));
const panels = readPanelVisibility(browserStorage(), { sidebar: true, explorer: window.innerWidth > 1100 });
const explorerWorkspaceId = ref(""), explorerPinned = ref(false), explorerOpen = ref(panels.explorer);
const sidebarExpanded = ref(panels.sidebar), sidebarRail = ref(sidebarIsRail(window.innerWidth));
/** A drawer-width viewport ignores the stored collapse without overwriting it. */
const sidebarCollapsed = computed(() => sidebarRail.value && !sidebarExpanded.value);
const explorerWorkspace = computed(() => workspaces.value.find(workspace => workspace.id === explorerWorkspaceId.value));
const activeFilePath = computed(() => focusedPane.value && focusedWorkspace.value?.id === explorerWorkspaceId.value ? paneString(focusedPane.value, "path") : undefined);
const sidebarOpen = ref(false), loading = ref(true), apiOnline = ref(false), error = ref("");
const connectionError = ref(""), refreshingResources = ref(false);
const visibleError = computed(() => error.value || connectionError.value);
const platform = ref<string>(), instanceLabel = ref<string>();
const showWorkspace = ref(false), showAuth = ref(false);
const settingsSection = ref<'agents' | 'endpoints' | 'accounts'>();
const archiveBusyIds = ref<string[]>([]), keepBusyIds = ref<string[]>([]), quickBusy = ref(false);
const ephemeralIds = computed(() => ephemeralSessionIds(sessions.value));
const discardPrompt = ref<{ paneId: string; session: Session }>();
const sessionNotice = ref("");
const enablingChat = new Set<string>();
const sessionWorkspaceId = ref<string>(), historyWorkspaceId = ref<string>(), sessionProvider = ref<ProviderKind>();
const environmentSessionId = ref<string>();
const renameSessionId = ref<string>();
const environmentSession = computed(() => sessions.value.find(session => session.id === environmentSessionId.value));
const renameSessionTarget = computed(() => sessions.value.find(session => session.id === renameSessionId.value));
const sessionWorkspace = computed(() => workspaces.value.find(workspace => workspace.id === sessionWorkspaceId.value));
const historyWorkspace = computed(() => workspaces.value.find(workspace => workspace.id === historyWorkspaceId.value));
const gitStatuses = ref<Record<string, GitStatus>>({}), gitAvailable = ref<Record<string, boolean>>({});
const fileRefresh = ref<Record<string, number>>({}), gitRefresh = ref<Record<string, number>>({});
const currentGit = computed<GitStatus>(() => gitStatuses.value[contextWorkspace.value?.id ?? ""] ?? { branch: null, files: [] });
const currentGitAvailable = computed(() => !!gitAvailable.value[contextWorkspace.value?.id ?? ""]);
const storage = browserStorage();
const storageKey = computed(() => canvasStorageKey(window.location.origin, workspaces.value));
const layoutStatus = ref("Saved"), canvasRevision = ref(0), canvasConflict = ref(false);
const localRecovery = ref<LayoutDocument>();
let suspendLayoutSave = true, disposed = false, bootstrapRevision = 0;
let layoutTimer: ReturnType<typeof setTimeout> | undefined, pollTimer: ReturnType<typeof setInterval> | undefined;
let pendingLayout: LayoutDocument | undefined;
let saveQueue: Promise<void> = Promise.resolve();
const sessionsLoading = ref(false);
let sessionsRevision = 0;
let workspaceRegistryRevision = 0;
const gitLoading = new Set<string>();
let wasDesktop = window.innerWidth > 1100;

function report(cause: unknown) {
  if (cause instanceof ApiConnectionError) {
    apiOnline.value = false;
    if (cause.outcomeUnknown) error.value = cause.message;
    else connectionError.value = cause.message;
    return;
  }
  if (cause instanceof ApiError && cause.status === 401) { showAuth.value = true; apiOnline.value = false; }
  else error.value = errorMessage(cause);
}
function rememberWorkspace(id: string) { try { storage?.setItem("agentdock:last-workspace", id); } catch { /* Optional preference. */ } }
function rememberedWorkspace() { try { return storage?.getItem("agentdock:last-workspace"); } catch { return null; } }
function copyLayout(document: LayoutDocument): LayoutDocument { return JSON.parse(JSON.stringify(document)); }
function sameLayout(a: LayoutDocument, b: LayoutDocument) { return sameCanvasLayout(a, b); }
function initialPane(node: LayoutNode): PaneNode | undefined {
  if (node.type === "pane") return node;
  if (node.type === "stack") return node.panes.find(pane => pane.id === node.activePaneId) ?? node.panes[0];
  return initialPane(node.first) ?? initialPane(node.second);
}
/** Everything that leaves this page — server canvas, browser cache, recovery
 * snapshot — drops temporary windows, so a reload cannot restore a pane whose
 * session the backend has already discarded. The live layout keeps them. */
function persistableLayout(document: LayoutDocument) { return withoutEphemeralPanes(document, sessions.value); }
function cacheLayout(document: LayoutDocument, pending: boolean) { return writeCanvasCache(storage, storageKey.value, persistableLayout(document), pending); }
function queueLayout(document: LayoutDocument) {
  const snapshot = persistableLayout(copyLayout(document));
  const cached = cacheLayout(snapshot, backendCapabilities.sharedCanvas);
  if (!backendCapabilities.sharedCanvas) { layoutStatus.value = cached ? "Saved in this browser" : "Memory only"; return; }
  pendingLayout = snapshot; layoutStatus.value = canvasConflict.value ? "Save conflict" : "Unsaved";
  clearTimeout(layoutTimer); layoutTimer = setTimeout(() => void flushLayout(), 450);
}
function persist(document: LayoutDocument) {
  saveQueue = saveQueue.then(async () => {
    if (canvasConflict.value || disposed) return;
    layoutStatus.value = "Saving…";
    try {
      const saved = await request<{ revision: number }>("/canvas/layout", json("PUT", { layout: document, expected_revision: canvasRevision.value }));
      canvasRevision.value = saved.revision;
      if (!pendingLayout && sameLayout(document, persistableLayout(layout.value))) { cacheLayout(document, false); layoutStatus.value = "Saved"; }
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 409) {
        canvasConflict.value = true; layoutStatus.value = "Save conflict";
      } else { layoutStatus.value = "Save failed"; report(cause); }
    }
  });
  return saveQueue;
}
async function flushLayout() {
  clearTimeout(layoutTimer);
  if (pendingLayout) { const document = pendingLayout; pendingLayout = undefined; void persist(document); }
  await saveQueue;
}
watch(layout, document => {
  if (suspendLayoutSave || !canvasReady.value) return;
  if (selectedPaneId.value && !flattenPanes(document.root).some(pane => pane.id === selectedPaneId.value)) selectedPaneId.value = initialPane(document.root)?.id;
  queueLayout(document);
}, { deep: true });

async function initializeCanvas(preferred: Workspace) {
  suspendLayoutSave = true;
  const local = readCanvasCache(storage, storageKey.value);
  let chosen: LayoutDocument | undefined, initialSave = false;
  if (backendCapabilities.sharedCanvas) {
    const shared = await request<SharedCanvas>("/canvas/layout");
    if (!Number.isSafeInteger(shared.revision) || shared.revision < 0 || (shared.layout !== null && !validateLayout(shared.layout))) throw new Error("Invalid shared canvas response");
    canvasRevision.value = shared.revision;
    if (shared.layout) {
      chosen = shared.layout;
      if (local?.pending && !sameLayout(local.layout, chosen)) {
        localRecovery.value = local.layout;
        writeCanvasCache(storage, storageKey.value + ":recovery", local.layout, true);
      } else localRecovery.value = readCanvasCache(storage, storageKey.value + ":recovery")?.layout;
    } else { chosen = local?.layout; initialSave = true; }
  } else chosen = local?.layout;
  if (!chosen) {
    const legacy = await request<LayoutDocument | null>(workspacePath(preferred.id) + "/layout");
    chosen = scopeLegacyLayout(legacy?.root && validateLayout(legacy) ? legacy : createDefaultLayoutDocument(), preferred.id, sessions.value);
    initialSave = true;
  }
  layout.value = copyLayout(chosen);
  selectedPaneId.value = initialPane(chosen.root)?.id;
  canvasReady.value = true; canvasConflict.value = false;
  explorerWorkspaceId.value = contextWorkspace.value?.id ?? preferred.id;
  await nextTick();
  suspendLayoutSave = false;
  if (!backendCapabilities.sharedCanvas || initialSave) queueLayout(layout.value);
  else layoutStatus.value = "Saved";
}
async function resolveCanvasConflict(useCurrent: boolean) {
  await flushLayout();
  try {
    const shared = await request<SharedCanvas>("/canvas/layout");
    if (!Number.isSafeInteger(shared.revision) || shared.revision < 0 || (shared.layout !== null && !validateLayout(shared.layout))) throw new Error("Invalid shared canvas response");
    canvasRevision.value = shared.revision; canvasConflict.value = false;
    if (useCurrent) queueLayout(layout.value);
    else if (shared.layout) {
      const backup = persistableLayout(copyLayout(layout.value));
      writeCanvasCache(storage, storageKey.value + ":recovery", backup, true); localRecovery.value = backup;
      suspendLayoutSave = true; layout.value = shared.layout; selectedPaneId.value = initialPane(shared.layout.root)?.id;
      await nextTick(); suspendLayoutSave = false; cacheLayout(layout.value, false); layoutStatus.value = "Saved";
    }
  } catch (cause) { report(cause); }
}
function restoreLocalRecovery() {
  if (!localRecovery.value) return;
  layout.value = copyLayout(localRecovery.value); selectedPaneId.value = initialPane(layout.value.root)?.id;
  localRecovery.value = undefined;
}
function selectWorkspace(id: string) {
  if (!workspaces.value.some(workspace => workspace.id === id)) return;
  selectedWorkspaceId.value = id; rememberWorkspace(id);
  if (!focusedWorkspace.value && !explorerPinned.value) explorerWorkspaceId.value = id;
  void refreshGit(id);
}
function selectPane(pane: PaneNode) {
  selectedPaneId.value = pane.id;
  const workspace = paneWorkspace(pane, workspaces.value, sessions.value);
  if (workspace) {
    if (!explorerPinned.value) explorerWorkspaceId.value = workspace.id;
    void refreshGit(workspace.id);
  }
}
watch(() => contextWorkspace.value?.id, id => { if (id && !explorerPinned.value) explorerWorkspaceId.value = id; });
// A prompt outlives neither its pane nor its record: a poll that already removed
// the session, or a pane closed elsewhere, must not leave a stale question.
watch([sessions, layout], () => {
  const prompt = discardPrompt.value;
  if (prompt && (!sessions.value.some(item => item.id === prompt.session.id) || !flattenPanes(layout.value.root).some(pane => pane.id === prompt.paneId))) discardPrompt.value = undefined;
}, { deep: true });
function openSession(session: Session) {
  if (!workspaces.value.some(workspace => workspace.id === session.workspace_id)) { error.value = t("Pane source unavailable"); return; }
  sessions.value = [session, ...sessions.value.filter(item => item.id !== session.id)];
  // Opening a session selects its bound view. A legacy PTY shown in the
  // conversation shell must not start just because a pane was mounted;
  // structured sessions and an explicit native-terminal view retain the
  // existing start intent.
  if (session.interaction_mode === "structured") sessionConnections.requestOpen(session.id);
  sidebarOpen.value = false;
  canvas.value?.openPane(sessionPane(session));
}
function sessionPaneDropped(pane: PaneNode) {
  const session = paneSession(pane, sessions.value);
  if (session && paneWorkspace(pane, workspaces.value, sessions.value) && session.interaction_mode === "structured") sessionConnections.requestOpen(session.id);
}
function acceptDroppedPane(pane: PaneNode) { return acceptsScopedPane(pane, workspaces.value, sessions.value); }
/**
 * Sidebar row -> canvas. Focus the view this session is already bound to, or
 * create its canonical one. Locating is navigation only: the stored record is
 * the source of truth for title and workspace, and no client is ever started.
 */
async function locateSession(session: Session) {
  const current = sessions.value.find(item => item.id === session.id);
  if (!current || !workspaces.value.some(workspace => workspace.id === current.workspace_id)) { error.value = t("Pane source unavailable"); return; }
  sidebarOpen.value = false;
  const bound = findSessionPaneId(layout.value.root, current.id);
  if (!bound) canvas.value?.openPane(sessionPane(current));
  await canvas.value?.focusPane(bound ?? sessionPane(current).id);
}
async function revealSession(id: string) {
  const session = sessions.value.find(item => item.id === id);
  if (!session) return;
  selectWorkspace(session.workspace_id);
  sidebarOpen.value = window.innerWidth <= 1100;
  await nextTick();
  await workspaceSidebar.value?.revealSession(id);
}
async function archiveSession(session: Session, archived: boolean) {
  if (!backendCapabilities.sessionArchive || archiveBusyIds.value.includes(session.id)) return;
  archiveBusyIds.value = [...archiveBusyIds.value, session.id];
  sessionsRevision++;
  sessionNotice.value = "";
  try {
    const updated = await request<Session>(`/sessions/${encodeURIComponent(session.id)}/archive`, json("PATCH", { archived }));
    sessionsRevision++;
    sessions.value = sessions.value.map(item => item.id === updated.id ? updated : item);
    sessionNotice.value = archived ? "Session archived. History and running agents are unchanged." : "Session restored to its workspace list.";
  } catch (cause) {
    report(cause);
  } finally {
    archiveBusyIds.value = archiveBusyIds.value.filter(id => id !== session.id);
  }
}
/** The one place where closing a view ends a process — allowed only because the
 * user chose "temporary window" when the session was created. */
async function discardEphemeral(session: Session): Promise<boolean> {
  try {
    await request<{ id: string }>(`/sessions/${encodeURIComponent(session.id)}`, json("DELETE"));
    sessionsRevision++;
    sessions.value = sessions.value.filter(item => item.id !== session.id);
    // Retire any open intent with the record, so nothing can reconnect it later.
    sessionConnections.ended(session.id);
    return true;
  } catch (cause) {
    // Already gone (a restart discards temporary sessions) is the wanted end
    // state, so the view may close; anything else keeps the pane and reports.
    if (cause instanceof ApiError && cause.status === 404 && !(cause instanceof ApiConnectionError)) {
      sessionsRevision++;
      sessions.value = sessions.value.filter(item => item.id !== session.id);
      sessionConnections.ended(session.id);
      return true;
    }
    report(cause); return false;
  }
}
/** Close guard. A non-ephemeral pane keeps its existing view-only behaviour and
 * issues no request at all; a working temporary session asks first. */
async function confirmClosePane(pane: PaneNode): Promise<boolean> {
  const session = paneSession(pane, sessions.value);
  if (!session || !isEphemeralSession(session)) return true;
  if (["running", "starting", "waiting"].includes(session.status)) { discardPrompt.value = { paneId: pane.id, session }; return false; }
  return await discardEphemeral(session);
}
async function confirmDiscard() {
  const prompt = discardPrompt.value;
  if (!prompt) return;
  discardPrompt.value = undefined;
  // The record is gone from `sessions` on success, so the guard now lets the
  // same pane close instead of prompting a second time.
  if (await discardEphemeral(prompt.session)) await canvas.value?.closePane(prompt.paneId);
}
async function keepSession(session: Session) {
  if (!isEphemeralSession(session) || keepBusyIds.value.includes(session.id)) return;
  keepBusyIds.value = [...keepBusyIds.value, session.id];
  sessionsRevision++;
  sessionNotice.value = "";
  try {
    const updated = await request<Session>(`/sessions/${encodeURIComponent(session.id)}/keep`, json("POST"));
    sessionsRevision++;
    sessions.value = sessions.value.map(item => item.id === updated.id ? updated : item);
    if (discardPrompt.value?.session.id === updated.id) discardPrompt.value = undefined;
    sessionNotice.value = "Session kept. It stays in the session list and closing its window no longer discards it.";
  } catch (cause) { report(cause); }
  finally { keepBusyIds.value = keepBusyIds.value.filter(id => id !== session.id); }
}
function keepSessionById(id: string) {
  const session = sessions.value.find(item => item.id === id);
  if (session) void keepSession(session);
}
async function renameSession(session: Session, title: string): Promise<boolean> {
  const nextTitle = title.trim();
  if (!nextTitle || nextTitle === session.title) return false;
  try {
    const updated = await request<Session>(`/sessions/${encodeURIComponent(session.id)}`, json("PATCH", { title: nextTitle }));
    sessions.value = sessions.value.map(item => item.id === updated.id ? updated : item);
    const nextRoot = renameSessionPanes(layout.value.root, updated.id, updated.title);
    if (nextRoot !== layout.value.root) layout.value = { ...layout.value, root: nextRoot };
    sessionNotice.value = "Session renamed. Its ID, history and running process are unchanged.";
    return true;
  } catch (cause) { report(cause); return false; }
}
function requestRenameSession(id: string) { if (sessions.value.some(session => session.id === id)) renameSessionId.value = id; }
async function saveRenameSession(title: string) {
  const session = renameSessionTarget.value;
  if (!session) return;
  if (await renameSession(session, title)) renameSessionId.value = undefined;
}
function sessionEnvironmentSaved(session: Session) {
  sessions.value = sessions.value.map(item => item.id === session.id ? session : item);
  environmentSessionId.value = undefined;
  // Updating metadata is not an open gesture: never requestOpen or start here.
}
async function enableChat(id: string) {
  if(enablingChat.has(id))return;enablingChat.add(id);
  try {const updated=await request<Session>('/sessions/'+encodeURIComponent(id)+'/conversation/open',json('POST',{}));sessions.value=sessions.value.map(session=>session.id===id?updated:session);}
  catch(cause){report(cause);}finally{enablingChat.delete(id);}
}
function openFile(id: string, file: FileEntry | string) {
  if (!workspaces.value.some(workspace => workspace.id === id)) return;
  canvas.value?.openPane(filePane(id, typeof file === "string" ? file : file.path));
  if (window.innerWidth <= 1100) explorerOpen.value = false;
}
function openChanges(id = contextWorkspace.value?.id) {
  if (!id || !workspaces.value.some(workspace => workspace.id === id)) return;
  canvas.value?.openPane(changesPane(id)); sidebarOpen.value = false;
}
function openFiles(id = selectedWorkspaceId.value) {
  if (!workspaces.value.some(workspace => workspace.id === id)) return;
  explorerWorkspaceId.value = id; explorerPinned.value = false; explorerOpen.value = true; sidebarOpen.value = false;
}
async function revealFile(id: string, path: string) {
  openFiles(id); await nextTick();
  try { await explorer.value?.reveal(path); } catch (cause) { report(cause); }
}
function newSession(id = selectedWorkspaceId.value, provider: ProviderKind = "claude_code") {
  if (!workspaces.value.some(workspace => workspace.id === id)) return;
  sessionWorkspaceId.value = id; sessionProvider.value = provider;
}
/**
 * One click from the sidebar to a usable session. The configuration dialog is
 * for the rare configured case; making every session pay for it is what made
 * opening a window feel heavy. Falls back to the dialog only when an account
 * genuinely cannot be chosen for the user.
 */
async function quickSession(id = selectedWorkspaceId.value, provider: ProviderKind = lastQuickProvider(), ephemeral = false, targetPaneId?: string) {
  if (!workspaces.value.some(workspace => workspace.id === id) || quickBusy.value) return;
  const plan = planQuickSession(provider, profiles.value, {
    ephemeral,
    structuredChat: backendCapabilities.structuredChat,
    ephemeralSupported: backendCapabilities.ephemeralSessions,
    existingTitles: sessions.value.filter(session => session.workspace_id === id).map(session => session.title),
  });
  if (plan.needsDialog || !plan.body) { newSession(id, provider); return; }
  quickBusy.value = true;
  try {
    const session = await request<Session>(`${workspacePath(id)}/sessions`, json("POST", plan.body));
    rememberQuickProvider(provider);
    sessionsRevision++;
    sessions.value = [session, ...sessions.value.filter(item => item.id !== session.id)];
    if (session.interaction_mode === "structured") sessionConnections.requestOpen(session.id);
    sidebarOpen.value = false;
    // A "+" pressed on a tab strip opens there; the sidebar has no target and
    // falls back to the canvas's own placement.
    canvas.value?.openPaneAt(targetPaneId, sessionPane(session));
  } catch (cause) { report(cause); }
  finally { quickBusy.value = false; }
}
/** Created from a tab strip's "+", so it lands in that stack. */
function createSessionInPane(targetId: string, provider: ProviderKind, ephemeral: boolean) {
  const pane = flattenPanes(layout.value.root).find(item => item.id === targetId);
  const workspaceId = (pane && paneString(pane, "workspace_id")) ?? contextWorkspace.value?.id ?? selectedWorkspaceId.value;
  void quickSession(workspaceId, provider, ephemeral, targetId);
}
function loadHistory(id: string) { if (backendCapabilities.nativeHistory) historyWorkspaceId.value = id; }
async function sessionCreated(session: Session) { sessionWorkspaceId.value = undefined; await nextTick(); openSession(session); }
async function historyLoaded(session: Session) { historyWorkspaceId.value = undefined; await sessionCreated(session); }
/** A worktree arrives as a workspace; the next thing wanted there is a session. */
async function worktreeOpened(workspace: Workspace) { await workspaceCreated(workspace); newSession(workspace.id); }
/** A worktree made while creating a session: registered without leaving the dialog. */
function worktreeWorkspace(workspace: Workspace) { workspaceRegistryRevision++; workspaces.value = [workspace, ...workspaces.value.filter(item => item.id !== workspace.id)]; }
async function workspaceCreated(workspace: Workspace) {
  workspaceRegistryRevision++;
  showWorkspace.value = false;
  workspaces.value = [workspace, ...workspaces.value.filter(item => item.id !== workspace.id)];
  selectWorkspace(workspace.id);
  if (!canvasReady.value) await initializeCanvas(workspace);
}
function tick(counter: typeof fileRefresh, id: string) { counter.value = { ...counter.value, [id]: (counter.value[id] ?? 0) + 1 }; }
function filesSaved(id: string) { tick(fileRefresh, id); tick(gitRefresh, id); void refreshGit(id); }
function gitChanged(id: string, status: GitStatus) { gitStatuses.value = { ...gitStatuses.value, [id]: status }; gitAvailable.value = { ...gitAvailable.value, [id]: true }; }
async function refreshGit(id: string) {
  if (!id || showAuth.value || gitLoading.has(id)) return;
  gitLoading.add(id);
  try {
    const result = await request<GitStatus>(workspacePath(id) + "/git/status");
    const changed = JSON.stringify(gitStatuses.value[id]) !== JSON.stringify(result);
    gitChanged(id, result);
    if (changed || activeSessions.value.some(session => session.workspace_id === id)) tick(gitRefresh, id);
  } catch (cause) { gitAvailable.value = { ...gitAvailable.value, [id]: false }; if (cause instanceof ApiConnectionError || (cause instanceof ApiError && cause.status === 401)) report(cause); }
  finally { gitLoading.delete(id); }
}
async function refreshSessions() {
  if (showAuth.value || sessionsLoading.value) return;
  sessionsLoading.value = true;
  const own = ++sessionsRevision;
  try { const list = await request<Session[]>("/sessions"); if (own === sessionsRevision) sessions.value = list; apiOnline.value = true; connectionError.value = ""; }
  catch (cause) { apiOnline.value = false; report(cause); }
  finally { sessionsLoading.value = false; }
}
let profileRefreshRevision = 0;
async function refreshProfiles(imported?: EndpointProfile) {
  const own = ++profileRefreshRevision;
  if (imported) profiles.value = [imported, ...profiles.value.filter(profile => profile.id !== imported.id)];
  try { const list = await request<EndpointProfile[]>("/endpoint-profiles"); if (own === profileRefreshRevision) profiles.value = list; }
  catch (cause) { if (own === profileRefreshRevision) report(cause); }
}
function trackedWorkspaces() {
  return new Set([contextWorkspace.value?.id, explorerWorkspace.value?.id, ...flattenPanes(layout.value.root).map(pane => paneWorkspace(pane, workspaces.value, sessions.value)?.id)].filter((id): id is string => !!id));
}
async function refreshResources() {
  if (refreshingResources.value || loading.value) return;
  refreshingResources.value = true; error.value = ""; connectionError.value = "";
  const registryRevision = workspaceRegistryRevision;
  try {
    const health = await request<BackendHealth>("/health");
    if (!health.ok) throw new ApiError("AgentDock health check failed", 503);
    const previousSharedStorage = backendCapabilities.sharedCanvas;
    setBackendCapabilities(health); apiOnline.value = true;
    platform.value = health.platform; instanceLabel.value = health.instance_label;
    const [workspaceList] = await Promise.all([request<Workspace[]>("/workspaces"), refreshSessions(), refreshProfiles()]);
    if (registryRevision === workspaceRegistryRevision) {
      workspaces.value = workspaceList;
      if (!workspaceList.some(workspace => workspace.id === selectedWorkspaceId.value)) selectedWorkspaceId.value = workspaceList[0]?.id ?? "";
    }
    if (selectedWorkspace.value && (!canvasReady.value || previousSharedStorage !== backendCapabilities.sharedCanvas)) {
      if (canvasReady.value) cacheLayout(layout.value, true);
      await initializeCanvas(selectedWorkspace.value);
    }
    await Promise.all([...trackedWorkspaces()].map(async id => { await refreshGit(id); tick(fileRefresh, id); tick(gitRefresh, id); }));
    if (layoutStatus.value === "Save failed" && !canvasConflict.value) queueLayout(layout.value);
  } catch (cause) { report(cause); }
  finally { refreshingResources.value = false; }
}
async function bootstrap() {
  const own = ++bootstrapRevision;
  loading.value = true; error.value = ""; connectionError.value = "";
  try {
    const auth = await request<{ required: boolean; authenticated: boolean }>("/auth");
    if (auth.required && !auth.authenticated) { showAuth.value = true; return; }
    showAuth.value = false;
    const [workspaceList, sessionList, profileList, health] = await Promise.all([
      request<Workspace[]>("/workspaces"), request<Session[]>("/sessions"),
      request<EndpointProfile[]>("/endpoint-profiles"), request<BackendHealth>("/health"),
    ]);
    if (disposed || own !== bootstrapRevision) return;
    apiOnline.value = health.ok; platform.value = health.platform; instanceLabel.value = health.instance_label;
    setBackendCapabilities(health);
    workspaces.value = workspaceList; sessions.value = sessionList; profiles.value = profileList;
    const preferred = workspaceList.find(workspace => workspace.id === selectedWorkspaceId.value || workspace.id === rememberedWorkspace()) ?? workspaceList[0];
    if (preferred) {
      selectedWorkspaceId.value = preferred.id;
      if (!canvasReady.value) await initializeCanvas(preferred);
      for (const id of trackedWorkspaces()) void refreshGit(id);
    }
  } catch (cause) { apiOnline.value = false; report(cause); }
  finally { if (own === bootstrapRevision) loading.value = false; }
}
function adaptDrawers() { const desktop = window.innerWidth > 1100; if (wasDesktop && !desktop) { explorerOpen.value = false; sidebarOpen.value = false; } wasDesktop = desktop; sidebarRail.value = sidebarIsRail(window.innerWidth); }
/** Below the rail width the same control drives the drawer, so the remembered
 * desktop preference is left untouched. */
function toggleSidebar() { if (sidebarRail.value) sidebarExpanded.value = !sidebarExpanded.value; else sidebarOpen.value = !sidebarOpen.value; }
// Persist wherever the panels change, including opening the explorer by
// revealing a file, so the shell reopens the way the user left it.
watch([sidebarExpanded, explorerOpen], ([sidebar, explorer]) => writePanelVisibility(storage, { sidebar, explorer }));
function beforeUnload(event: BeforeUnloadEvent) {
  if (hasDirtyDrafts() || hasGitDrafts() || pendingLayout || ["Saving…", "Save failed", "Save conflict", "Memory only"].includes(layoutStatus.value)) { event.preventDefault(); event.returnValue = ""; }
}
/**
 * Give way to a phone keyboard.
 *
 * `100dvh` counts the browser's own chrome and nothing else, so a keyboard
 * covers the bottom of the page without the page ever knowing. Whatever lives
 * down there goes with it: in a terminal that is the prompt, and the command
 * list a client draws underneath it, which was the whole of what a slash did.
 */
function fitToKeyboard() {
  const height = shellHeight(window.visualViewport ?? undefined, window.innerHeight);
  document.documentElement.style.setProperty("--app-height", height ? `${height}px` : "");
}
onMounted(() => {
  void bootstrap();
  pollTimer = setInterval(() => { if (!document.hidden && !showAuth.value) { void refreshSessions(); for (const id of trackedWorkspaces()) void refreshGit(id); } }, 5000);
  window.addEventListener("beforeunload", beforeUnload); window.addEventListener("resize", adaptDrawers);
  // The visual viewport also scrolls under a keyboard, which is the other way
  // its height stops describing what can be seen.
  window.visualViewport?.addEventListener("resize", fitToKeyboard);
  window.visualViewport?.addEventListener("scroll", fitToKeyboard);
  fitToKeyboard();
});
onUnmounted(() => { if (pendingLayout) cacheLayout(pendingLayout, true); disposed = true; clearInterval(pollTimer); clearTimeout(layoutTimer); window.removeEventListener("beforeunload", beforeUnload); window.removeEventListener("resize", adaptDrawers); window.visualViewport?.removeEventListener("resize", fitToKeyboard); window.visualViewport?.removeEventListener("scroll", fitToKeyboard); });
</script>

<template>
  <div :class="['app-shell', { 'explorer-hidden': !explorerOpen || !explorerWorkspace, 'sidebar-collapsed': sidebarCollapsed }]">
    <header class="topbar">
      <div class="brand"><button class="icon-button mobile-menu" :aria-label="t('Toggle workspace navigation')" @click="sidebarOpen = !sidebarOpen"><Icon name="menu" /></button><button class="icon-button sidebar-toggle" :class="{selected:!sidebarCollapsed}" :aria-pressed="!sidebarCollapsed" :aria-label="t(sidebarCollapsed?'Expand workspace panel':'Collapse workspace panel')" :title="t(sidebarCollapsed?'Expand workspace panel':'Collapse workspace panel')" @click="toggleSidebar"><Icon name="panelLeft"/></button><span class="brand-mark"><span /></span><strong>AgentDock<span class="brand-version">{{ instanceLabel || t('local / mvp') }}</span></strong></div>
      <div class="top-crumb"><span>{{ t('Shared workspace canvas') }}</span><Icon name="chevron" :size="13"/><strong>{{ contextWorkspace?.name || t('Your next workspace') }}</strong></div>
      <div class="top-actions">
        <span class="connection-badge"><i :class="['state-dot', apiOnline ? 'running' : 'stopped']"/>{{ t(loading ? 'Connecting' : apiOnline ? 'Host connected' : 'Offline') }}</span>
        <button v-if="workspaces.length" class="icon-button workspace-refresh-top" :aria-label="t('Refresh workspace')" :aria-busy="refreshingResources" :title="t(refreshingResources?'Refreshing workspace data…':'Refresh workspace')" :disabled="loading||refreshingResources" @click="refreshResources"><Icon name="refresh" :size="16"/></button>
        <button v-if="workspaces.length" class="primary-button new-session-top" :disabled="!selectedWorkspace" :aria-label="t('New session')" :title="selectedWorkspace?t('New session in {workspace}',{workspace:selectedWorkspace.name}):t('New session')" @click="newSession()"><Icon name="plus" :size="14"/><span>{{ t('New session') }}</span></button>
        <button class="secondary-button add-workspace-top" @click="showWorkspace = true"><Icon name="plus" :size="14"/>{{ t('Workspace') }}</button>
        <select class="language-select" :value="locale" :aria-label="t('Language')" @change="setLocale(($event.target as HTMLSelectElement).value === 'en' ? 'en' : 'zh-CN')"><option value="zh-CN" lang="zh-CN">中文</option><option value="en" lang="en">English</option></select>
        <button class="icon-button" :aria-label="t('Settings')" :title="t('Settings')" @click="settingsSection='agents'"><Icon name="settings"/></button>
        <button v-if="workspaces.length" :class="['icon-button',{selected:explorerOpen}]" :aria-pressed="explorerOpen" :aria-label="t(explorerOpen?'Collapse file panel':'Expand file panel')" :title="t(explorerOpen?'Collapse file panel':'Expand file panel')" @click="explorerOpen = !explorerOpen"><Icon name="panelRight"/></button>
      </div>
    </header>
    <div class="app-body">
      <button v-if="sidebarOpen" class="drawer-overlay sidebar-overlay" :aria-label="t('Close workspace navigation')" @click="sidebarOpen = false"/>
      <aside :class="['sidebar',{'drawer-open':sidebarOpen}]">
        <WorkspaceSidebar ref="workspaceSidebar" :workspaces="workspaces" :sessions="sessions" :selected-workspace-id="selectedWorkspaceId" :selected-session-id="selectedSessionId" :history-supported="backendCapabilities.nativeHistory" :storage-key="storageKey" :archive-supported="backendCapabilities.sessionArchive" :ephemeral-supported="backendCapabilities.ephemeralSessions" :archive-busy-ids="archiveBusyIds" :keep-busy-ids="keepBusyIds" :sessions-loading="sessionsLoading" @select-workspace="selectWorkspace" @open-session="openSession" @locate-session="locateSession" @rename-session="renameSession" @archive-session="archiveSession" @keep-session="keepSession" @refresh-sessions="refreshSessions" @session-environment="environmentSessionId=$event;sidebarOpen=false" @open-files="openFiles" @open-changes="openChanges" @new-session="newSession" @quick-session="quickSession" @load-history="loadHistory" @add-workspace="showWorkspace=true" @canvas="sidebarOpen=false"/>
        <div class="host-card"><span class="host-symbol"><Icon name="terminal"/></span><div><strong>{{ t('Host native') }}</strong><small>{{ platform || 'macOS / Linux' }} · {{ t('no containers') }}</small></div><span :class="['state-dot',apiOnline?'running':'stopped']"/></div>
      </aside>
      <main class="main-workspace">
        <div v-if="visibleError" class="app-error" role="alert"><span>{{ visibleError }}</span><button class="text-button" :disabled="refreshingResources||loading" @click="canvasReady?refreshResources():bootstrap()">{{ t('Retry') }}</button><button class="icon-button" :aria-label="t('Dismiss error')" @click="error='';connectionError=''"><Icon name="close" :size="14"/></button></div>
        <div v-if="sessionNotice" class="canvas-compat-notice" role="status"><Icon name="check" :size="14"/><span>{{ t(sessionNotice) }}</span><button class="icon-button" :aria-label="t('Dismiss session notice')" @click="sessionNotice=''"><Icon name="close" :size="13"/></button></div>
        <div v-if="canvasReady && !backendCapabilities.sharedCanvas" class="canvas-compat-notice" role="status"><Icon name="info" :size="15"/><span>{{ t('This backend is older. The shared canvas is saved in this browser; running sessions and existing workspace layouts are unchanged.') }}<small>{{ t('Native configuration import and history loading require an updated backend.') }}</small></span></div>
        <div v-if="discardPrompt" class="confirmation-bar" role="alert">{{ t('“{session}” is a temporary window and is still working. Closing it discards the session and stops it.',{session:discardPrompt.session.title}) }}<button class="small-button" @click="discardPrompt=undefined">{{ t('Keep it open') }}</button><button class="small-button danger" @click="confirmDiscard">{{ t('Close and discard') }}</button></div>
        <div v-if="canvasConflict" class="confirmation-bar" role="alert">{{ t('Another page updated the shared canvas. Your current layout is kept locally.') }}<button class="small-button" @click="resolveCanvasConflict(false)">{{ t('Load server layout') }}</button><button class="small-button danger" @click="resolveCanvasConflict(true)">{{ t('Save my current layout instead') }}</button></div>
        <div v-if="localRecovery" class="canvas-compat-notice"><span>{{ t('A local recovery layout is available.') }}</span><button class="text-button" @click="restoreLocalRecovery">{{ t('Restore local layout') }}</button><button class="icon-button" :aria-label="t('Dismiss error')" @click="localRecovery=undefined"><Icon name="close" :size="13"/></button></div>
        <template v-if="workspaces.length">
          <Canvas v-if="canvasReady" ref="canvas" v-model="layout" :selected-pane-id="selectedPaneId" :default-workspace-id="contextWorkspace?.id" :accept-pane="acceptDroppedPane" :confirm-close-pane="confirmClosePane" :ephemeral-session-ids="ephemeralIds" :ephemeral-supported="backendCapabilities.ephemeralSessions" @create-session="createSessionInPane" @reveal-session="revealSession" :workspace-labels="Object.fromEntries(workspaces.map(workspace=>[workspace.id,workspace.name]))" :workspace-branches="Object.fromEntries(Object.entries(gitStatuses).flatMap(([id,status])=>status.branch?[[id,status.branch]]:[]))" :session-providers="sessionProviders" @select-pane="selectPane" @open-pane="sessionPaneDropped">
            <template #pane="{ pane }"><WorkspacePane :key="pane.id" :pane="pane" :workspaces="workspaces" :sessions="sessions" :profiles="profiles" :git-refresh="gitRefresh" @session-changed="refreshSessions" @new-session="(id,provider)=>newSession(id??selectedWorkspaceId,provider)" @open-session="openSession" @reveal-session="revealSession" @keep-session="keepSessionById" @rename-request="requestRenameSession" @session-environment="environmentSessionId=$event" @structured-session="enableChat" @profiles="settingsSection='endpoints'" @git-changed="gitChanged" @open-file="openFile" @files-saved="filesSaved" @browse="id=>openFiles(id??selectedWorkspaceId)" @reveal="revealFile" @worktree="worktreeOpened"/></template>
          </Canvas>
          <div v-else class="pane-empty"><p>{{ t('Opening workspace…') }}</p></div>
        </template>
        <section v-else class="welcome-screen"><div class="welcome-composition" aria-hidden="true"><span class="welcome-tile"><Icon name="spark" :size="35"/></span><span class="welcome-tile"><Icon name="git" :size="26"/><Icon name="file" :size="26"/></span></div><div class="eyebrow">{{ t('A LITTLE SPACE. ENDLESS POSSIBILITIES.') }}</div><h1>{{ t('Your agents.') }}<br/>{{ t('Your workspace.') }}</h1><p>{{ t('Claude Code, Codex, files and Git — together on your own machine. Arrange everything the way you think.') }}</p><button class="primary-button" :disabled="loading" @click="showWorkspace=true"><Icon name="plus" :size="17"/>{{ t(loading?'Connecting to host…':'Add a workspace') }}</button><small>{{ t('Rust + Vue · host native · browser first') }}</small></section>
      </main>
      <button v-if="explorerOpen && explorerWorkspace" class="drawer-overlay explorer-overlay" :aria-label="t('Close file explorer')" @click="explorerOpen=false"/>
      <aside v-if="explorerOpen && explorerWorkspace" class="explorer-panel">
        <div class="explorer-workspace-control"><label><span>{{ t('Explorer workspace') }}</span><select :value="explorerWorkspaceId" :aria-label="t('Explorer workspace')" @change="explorerWorkspaceId=($event.target as HTMLSelectElement).value;explorerPinned=true"><option v-for="workspace in workspaces" :key="workspace.id" :value="workspace.id">{{ workspace.name }}</option></select></label><button class="small-button" :class="{primary:explorerPinned}" :aria-pressed="explorerPinned" :title="t(explorerPinned?'Unpin explorer and follow the focused pane':'Pin explorer to this workspace')" @click="explorerPinned=!explorerPinned;!explorerPinned&&contextWorkspace&&(explorerWorkspaceId=contextWorkspace.id)">{{ t(explorerPinned?'Pinned':'Follow focus') }}</button></div>
        <FileExplorer ref="explorer" :workspace-id="explorerWorkspace.id" :refresh-token="fileRefresh[explorerWorkspace.id]??0" :selected-path="activeFilePath" @open="openFile(explorerWorkspace.id,$event)" @close="explorerOpen=false"/>
      </aside>
    </div>
    <footer class="statusbar"><span :title="contextWorkspace?.root_path"><Icon name="git" :size="13"/>{{ contextWorkspace?.name || t('No workspace') }} · {{ currentGitAvailable?currentGit.branch||t('Detached HEAD'):t('Git unavailable') }}</span><button v-if="contextWorkspace" @click="openChanges(contextWorkspace.id)">{{ currentGitAvailable?t('{count} changes',{count:currentGit.files.length}):t('Changes') }}</button><span v-if="currentGit.ahead!==undefined">↑ {{ currentGit.ahead }}</span><span v-if="currentGit.behind!==undefined">↓ {{ currentGit.behind }}</span><span class="flex-spacer"/><span v-if="canvasReady" :class="{'danger-text':['Save failed','Save conflict','Memory only'].includes(layoutStatus)}">{{ t('Layout {status}',{status:t(layoutStatus)}) }}</span><HostUsage :sessions="sessions" /><span class="status-agent-count"><i :class="['state-dot',activeSessions.length?'running':'stopped']"/>{{ t('{count} active sessions',{count:activeSessions.length}) }}</span></footer>
  </div>
  <WorkspaceDialog v-if="showWorkspace" @close="showWorkspace=false" @created="workspaceCreated"/>
  <ImageLightbox />
  <SettingsDialog v-if="settingsSection" :profiles="profiles" :initial-section="settingsSection" @close="settingsSection=undefined" @changed="refreshProfiles"/>
  <CreateSessionDialog v-if="sessionWorkspace" :workspace="sessionWorkspace" :profiles="profiles" :initial-provider="sessionProvider" @close="sessionWorkspaceId=undefined" @created="sessionCreated" @workspace="worktreeWorkspace" @profiles="sessionWorkspaceId=undefined;settingsSection='endpoints'"/>
  <SessionEnvironmentDialog v-if="environmentSession" :key="environmentSession.id" :session="environmentSession" @close="environmentSessionId=undefined" @saved="sessionEnvironmentSaved"/>
  <SessionRenameDialog v-if="renameSessionTarget" :key="renameSessionTarget.id" :session="renameSessionTarget" @close="renameSessionId=undefined" @save="saveRenameSession"/>
  <LoadHistoryDialog v-if="historyWorkspace && backendCapabilities.nativeHistory" :workspace="historyWorkspace" @close="historyWorkspaceId=undefined" @loaded="historyLoaded"/>
  <AuthDialog v-if="showAuth" @authenticated="bootstrap"/>
</template>

<style scoped>
.language-select{color:#607681;border:1px solid #e1e8e9;border-radius:6px;background:#f9fbfa;font-size:10px;padding:5px 3px 5px 7px;min-height:28px;cursor:pointer}
.new-session-top{padding:5px 9px;font-size:10px;min-height:30px}.top-actions{gap:9px}.main-workspace{padding:10px 12px 0}.canvas-compat-notice>span{flex:1;min-width:0}
.canvas-compat-notice{display:flex;align-items:center;gap:8px;flex:none;padding:8px 11px;margin-bottom:10px;background:#f1f7f4;border:1px solid #dce8e1;border-radius:7px;font-size:11px;line-height:17px;color:#628276}.canvas-compat-notice>svg{flex:none}.canvas-compat-notice small{display:block;color:#8a9d94;font-size:10px}
.explorer-workspace-control{display:flex;align-items:end;gap:6px;padding:10px;border-bottom:1px solid #e6ece9;background:#f8fbf9;flex:none}.explorer-workspace-control label{min-width:0;flex:1;font-size:9px;color:#8b9f94}.explorer-workspace-control label>span{display:block;margin-bottom:4px}.explorer-workspace-control select{width:100%;font-size:11px;padding:5px;border:1px solid #dce6df;border-radius:5px;background:white;color:#5d7868}.explorer-workspace-control button{white-space:nowrap;font-size:9px}
@media(max-width:700px){.canvas-compat-notice{font-size:10px}.canvas-compat-notice small{display:none}.workspace-heading h1{font-size:19px}.workspace-heading .branch-badge{display:none}}
@media(max-width:380px){.language-select{font-size:9px;width:60px;padding-left:4px}}
@media(max-width:1100px){.top-crumb>span,.top-crumb>svg{display:none}.connection-badge{display:none}}
@media(max-width:700px){.main-workspace{padding:6px 6px 0}.new-session-top>span{display:none}.new-session-top{width:30px;padding:5px}.workspace-refresh-top{display:none}.top-actions{gap:5px}}
</style>
