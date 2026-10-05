<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import type { Session } from "@agentdock/protocol";
import { useI18n } from "../i18n";
import { providerLabel } from "./api";
import { isEphemeralSession, isSessionArchived } from "./session-list";
import { workspaceSessionPane } from "./workspace-groups";
import Icon from "./Icon.vue";
import ProviderIcon from "./ProviderIcon.vue";

const props = withDefaults(defineProps<{
  session: Session;
  selected?: boolean;
  workspaceName?: string;
  archiveSupported?: boolean;
  archiveBusy?: boolean;
  keepBusy?: boolean;
  deleteBusy?: boolean;
  /** Shown with a checkbox, for acting on several sessions at once. */
  selectable?: boolean;
  checked?: boolean;
}>(), { selected: false, archiveSupported: false, archiveBusy: false, keepBusy: false, deleteBusy: false, selectable: false, checked: false });
const emit = defineEmits<{
  open: [session: Session];
  check: [session: Session, range: boolean];
  delete: [session: Session];
  environment: [id: string];
  rename: [session: Session, title: string];
  archive: [session: Session, archived: boolean];
  keep: [session: Session];
}>();
const { t } = useI18n();
const menuOpen = ref(false);
const menuButton = ref<HTMLButtonElement>();
const confirmDelete = ref(false);
/** Deleting is offered only where the server allows it: a temporary session,
 * or a permanent one already archived out of its workspace list. */
const deletable = computed(() => isEphemeralSession(props.session) || isSessionArchived(props.session));
const renaming = ref(false), titleDraft = ref(props.session.title), renameInput = ref<HTMLInputElement>();

/**
 * The provider icon already sits next to the title, so repeating "Claude Code"
 * here said nothing. Inside a workspace group the parent names the workspace,
 * leaving last-activity as the one genuinely useful detail for the row.
 */
const lastActive = computed(() => {
  const at = Date.parse(props.session.updated_at ?? "");
  if (!Number.isFinite(at)) return t("Not run yet");
  const minutes = Math.round((Date.now() - at) / 60000);
  if (minutes < 1) return t("just now");
  if (minutes < 60) return t("{n} min ago", { n: minutes });
  const hours = Math.round(minutes / 60);
  if (hours < 24) return t("{n} h ago", { n: hours });
  return t("{n} d ago", { n: Math.round(hours / 24) });
});
function dragSession(event: DragEvent) {
  event.dataTransfer?.setData("application/agentdock-pane", JSON.stringify(workspaceSessionPane(props.session)));
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copyMove";
}
function toggleMenu() { menuOpen.value = !menuOpen.value; confirmDelete.value = false; }
/** Right-clicking a row opens the same menu its ··· button does, so the two
 * cannot drift into offering different actions. */
async function openMenuFromPointer(event: MouseEvent) {
  if (renaming.value) return;
  event.preventDefault();
  menuOpen.value = true; confirmDelete.value = false;
  // A right-click does not move focus, and this menu is dismissed by focus
  // leaving it. Focusing the button it belongs to restores that.
  await nextTick();
  menuButton.value?.focus();
}
async function closeMenu(restoreFocus = false) {
  menuOpen.value = false;
  if (restoreFocus) { await nextTick(); menuButton.value?.focus(); }
}
function escapeMenu(event: KeyboardEvent) {
  if (event.key !== "Escape" || !menuOpen.value) return;
  event.preventDefault(); event.stopPropagation(); void closeMenu(true);
}
function leaveMenu(event: FocusEvent) {
  if (event.relatedTarget instanceof Node && event.currentTarget instanceof Node && event.currentTarget.contains(event.relatedTarget)) return;
  menuOpen.value = false;
}
function archiveSession() {
  if (!props.archiveSupported || props.archiveBusy) return;
  emit("archive", props.session, !isSessionArchived(props.session));
  void closeMenu(true);
}
function deleteSession() {
  if (!deletable.value || props.deleteBusy) return;
  emit("delete", props.session); confirmDelete.value = false; void closeMenu(true);
}
function environment() { emit("environment", props.session.id); void closeMenu(true); }
function keepSession() {
  if (!isEphemeralSession(props.session) || props.keepBusy) return;
  emit("keep", props.session); void closeMenu(true);
}
function beginRename() {
  titleDraft.value = props.session.title;
  menuOpen.value = false;
  renaming.value = true;
  void nextTick(() => { renameInput.value?.focus(); renameInput.value?.select(); });
}
function cancelRename() { renaming.value = false; titleDraft.value = props.session.title; }
function submitRename() {
  const title = titleDraft.value.trim();
  if (!title) { renameInput.value?.focus(); return; }
  renaming.value = false;
  emit("rename", props.session, title);
}
</script>

<template>
  <li class="sidebar-session-row" :class="{ selected, checked, archived: isSessionArchived(session) }" :data-sidebar-session-id="session.id" @contextmenu="openMenuFromPointer" @keydown="escapeMenu" @focusout="leaveMenu">
    <div class="sidebar-session-main">
      <label v-if="selectable" class="sidebar-session-check" :title="t('Select session')" @click.stop><input type="checkbox" :checked="checked" :aria-label="t('Select {session}', { session: session.title })" @click.prevent="emit('check', session, $event.shiftKey)" /></label>
      <button class="workspace-session" :class="{ active: selected }" :aria-current="selected ? 'true' : undefined" :title="`${session.title}\n${workspaceName ? `${workspaceName} · ` : ''}${providerLabel(session.provider)} · ${t(session.status)}`" draggable="true" @dragstart="dragSession" @click="emit('open', session)">
        <span :class="['workspace-session-provider', session.provider]"><ProviderIcon :provider="session.provider" :size="14" /></span>
        <strong class="workspace-session-title">{{ session.title }}</strong>
        <small class="workspace-session-meta"><Icon v-if="isSessionArchived(session)" name="archive" :size="11" :aria-label="t('Archived session')" /><span v-if="isEphemeralSession(session)" class="session-temporary-badge">{{ t('Temporary') }}</span><span v-if="session.status === 'waiting'" class="session-waiting-badge">{{ t('Needs you') }}</span><span v-else-if="session.status !== 'stopped'" :class="['state-dot', session.status]" :title="t(session.status)" :aria-label="t(session.status)" /><span class="workspace-session-when">{{ workspaceName || lastActive }}</span></small>
      </button>
      <div class="sidebar-session-actions">
        <button ref="menuButton" class="icon-button session-more-button" :class="{ selected: menuOpen }" :aria-label="t('Session actions for {session}', { session: session.title })" :aria-expanded="menuOpen" :title="t('Session actions')" @click="toggleMenu"><Icon name="more" :size="15" /></button>
      </div>
    </div>
    <form v-if="renaming" class="sidebar-session-rename" @submit.prevent="submitRename" @keydown.esc.prevent.stop="cancelRename">
      <input ref="renameInput" v-model="titleDraft" :aria-label="t('Session title')" :placeholder="t('Session title')" maxlength="200" />
      <button type="submit" :aria-label="t('Save session name')" :title="t('Save session name')"><Icon name="check" :size="12" /></button>
      <button type="button" :aria-label="t('Cancel rename')" :title="t('Cancel rename')" @click="cancelRename"><Icon name="close" :size="12" /></button>
    </form>
    <div v-else-if="menuOpen" class="sidebar-session-menu" role="menu" :aria-label="t('Session actions')">
      <button class="session-rename-action" role="menuitem" @click="beginRename"><Icon name="edit" :size="13" />{{ t('Rename session') }}</button>
      <button class="session-environment-action" role="menuitem" @click="environment"><Icon name="settings" :size="13" />{{ t('Session environment') }}</button>
      <button v-if="isEphemeralSession(session)" class="session-keep-action" role="menuitem" :disabled="keepBusy" :aria-busy="keepBusy" :title="t('Keep this temporary session permanently. It stays in the session list and closing its window no longer discards it.')" @click="keepSession"><Icon name="check" :size="13" />{{ t(keepBusy ? 'Saving…' : 'Keep this session') }}</button>
      <hr />
      <button class="session-archive-action" role="menuitem" :disabled="!archiveSupported || archiveBusy" :aria-busy="archiveBusy" :title="!archiveSupported ? t('Upgrade the backend to archive sessions.') : t(isSessionArchived(session) ? 'Restore this session to its workspace list.' : 'Archive hides this session from workspace lists without stopping it.')" @click="archiveSession"><Icon :name="isSessionArchived(session) ? 'restore' : 'archive'" :size="13" />{{ t(archiveBusy ? 'Saving…' : isSessionArchived(session) ? 'Restore session' : 'Archive session') }}</button>
      <small v-if="!archiveSupported">{{ t('Upgrade the backend to archive sessions.') }}</small>
      <template v-if="deletable">
        <template v-if="confirmDelete">
          <p class="session-delete-confirm">{{ t('Delete {session}? Its record and conversation are removed and cannot be restored.', { session: session.title }) }}</p>
          <button class="session-delete-action" role="menuitem" :disabled="deleteBusy" :aria-busy="deleteBusy" @click="deleteSession"><Icon name="close" :size="13" />{{ t(deleteBusy ? 'Deleting…' : 'Delete permanently') }}</button>
          <button role="menuitem" @click="confirmDelete = false">{{ t('Cancel') }}</button>
        </template>
        <button v-else class="session-delete-action" role="menuitem" :disabled="deleteBusy" @click="confirmDelete = true"><Icon name="close" :size="13" />{{ t('Delete session…') }}</button>
      </template>
    </div>
  </li>
</template>

<style scoped>
.sidebar-session-row{min-width:0;list-style:none;scroll-margin-block:14px;border-radius:var(--radius-sm)}.sidebar-session-main{display:flex;align-items:center;min-width:0}.sidebar-session-main{position:relative}.workspace-session{display:flex;align-items:center;gap:8px;flex:1;min-width:0;border:0;border-radius:var(--radius-sm);padding:7px 8px;margin:1px 0;background:none;text-align:left}.workspace-session:hover{background:var(--fill)}.workspace-session.active{background:var(--teal-soft)}.workspace-session-provider{display:grid;place-items:center;flex-shrink:0;width:16px;color:#c0835a}.workspace-session-provider.codex{color:var(--violet)}.workspace-session-provider.terminal{color:var(--muted)}.workspace-session-title{flex:1;min-width:0;font-size:var(--text-md);font-weight:500;color:var(--ink);line-height:18px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.workspace-session.active .workspace-session-title{color:var(--teal);font-weight:600}.workspace-session-meta{display:flex;align-items:center;gap:5px;flex:none;max-width:45%;font-size:var(--text-xs);line-height:16px;color:var(--faint);white-space:nowrap}.workspace-session-when{overflow:hidden;text-overflow:ellipsis}.workspace-session-meta>svg{flex:none;color:#ad8d60}.workspace-session-meta>.state-dot{width:7px;height:7px}.sidebar-session-actions{position:absolute;right:3px;top:50%;transform:translateY(-50%);display:flex;align-items:center;gap:1px;opacity:0;pointer-events:none;border-radius:var(--radius-sm);background:var(--fill)}.sidebar-session-row:hover .sidebar-session-actions,.sidebar-session-main:focus-within .sidebar-session-actions,.sidebar-session-actions:has(.selected){opacity:1;pointer-events:auto}.sidebar-session-row:hover .workspace-session-meta,.sidebar-session-main:focus-within .workspace-session-meta{visibility:hidden}.sidebar-session-row.selected .sidebar-session-actions{background:var(--teal-soft)}.sidebar-session-actions>.icon-button{width:26px;height:26px;padding:4px}.sidebar-session-check{display:grid;place-items:center;flex:none;width:20px;height:28px;cursor:pointer}.sidebar-session-check>input{width:12px;height:12px;margin:0;accent-color:var(--teal);cursor:pointer}.sidebar-session-row.checked>.sidebar-session-main>.workspace-session{background:var(--teal-soft)}.session-more-button{font-size:var(--text-xl);line-height:18px;color:var(--muted)}.session-more-button:hover,.session-more-button.selected{color:var(--ink);background:var(--fill)}.sidebar-session-menu{margin:0 2px 6px 27px;padding:4px;border:1px solid var(--fill-hover);border-radius:var(--radius-sm);background:var(--surface)}.sidebar-session-menu>button{display:flex;align-items:center;gap:6px;width:100%;border:0;border-radius:var(--radius-xs);background:none;padding:6px 5px;text-align:left;font-size:var(--text-xs);line-height:15px;color:var(--ink-soft)}.sidebar-session-menu>button>svg{flex:none}.sidebar-session-menu>button:hover:not(:disabled){background:var(--fill);color:var(--ink)}.sidebar-session-menu>button:disabled{opacity:.55;cursor:not-allowed}.session-temporary-badge{flex:none;border-radius:var(--radius-xs);padding:0 4px;background:var(--violet-soft);color:var(--violet);font-size:var(--text-xs);line-height:13px}.session-keep-action>svg{color:var(--violet)}.session-environment-action>svg{color:var(--info)}.session-archive-action>svg{color:#ad8d60}.archived .session-archive-action>svg{color:var(--ok-ink)}.sidebar-session-menu>hr{border:0;border-top:1px solid var(--fill);margin:3px 4px}.session-delete-action{color:var(--danger)!important}.session-delete-action>svg{color:var(--danger)}.session-delete-action:hover:not(:disabled){background:var(--danger-soft)!important}.session-delete-confirm{margin:2px 5px 3px;font-size:var(--text-xs);line-height:13px;color:var(--danger-ink)}.sidebar-session-menu>small{display:block;padding:2px 5px;font-size:var(--text-xs);line-height:14px;color:#9c875f}
.session-rename-action>svg{color:var(--info)}.sidebar-session-rename{display:flex;align-items:center;gap:4px;margin:2px 2px 6px 27px;padding:3px;border:1px solid var(--ok-line);border-radius:var(--radius-sm);background:var(--sunken)}.sidebar-session-rename input{min-width:0;flex:1;border:0;background:transparent;padding:5px 4px;font-size:var(--text-xs);color:var(--ink-soft);outline:0}.sidebar-session-rename button{display:grid;place-items:center;width:26px;height:26px;border:0;border-radius:var(--radius-sm);background:none;color:var(--ink-soft);cursor:pointer}.sidebar-session-rename button:first-of-type{color:var(--accent-ink)}.sidebar-session-rename button:hover{background:var(--accent-soft)}
@media(pointer:coarse){.workspace-session{min-height:44px}.sidebar-session-actions>.icon-button{width:34px;height:44px}.sidebar-session-menu>button{min-height:44px;font-size:var(--text-sm)}.sidebar-session-rename{min-height:44px}.sidebar-session-rename input{font-size:var(--input-text)}.sidebar-session-rename button{width:36px;height:36px}.sidebar-session-actions{position:static;transform:none;opacity:1;pointer-events:auto;background:none}.sidebar-session-row:hover .workspace-session-meta,.sidebar-session-main:focus-within .workspace-session-meta{visibility:visible}.workspace-session-title{font-size:var(--text-lg)}.workspace-session-meta{font-size:var(--text-sm)}}
.session-waiting-badge{flex:none;padding:0 6px;border-radius:var(--radius-full);background:var(--warn-soft);color:var(--warn-ink);font-size:var(--text-xs);font-weight:600;line-height:18px}
</style>
