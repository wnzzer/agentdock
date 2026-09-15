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
}>(), { selected: false, archiveSupported: false, archiveBusy: false, keepBusy: false });
const emit = defineEmits<{
  open: [session: Session];
  locate: [session: Session];
  environment: [id: string];
  rename: [session: Session, title: string];
  archive: [session: Session, archived: boolean];
  keep: [session: Session];
}>();
const { t } = useI18n();
const menuOpen = ref(false);
const menuButton = ref<HTMLButtonElement>();
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
function toggleMenu() { menuOpen.value = !menuOpen.value; }
/** Right-clicking a row opens the same menu its ··· button does, so the two
 * cannot drift into offering different actions. */
async function openMenuFromPointer(event: MouseEvent) {
  if (renaming.value) return;
  event.preventDefault();
  menuOpen.value = true;
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
  <li class="sidebar-session-row" :class="{ selected, archived: isSessionArchived(session) }" :data-sidebar-session-id="session.id" @contextmenu="openMenuFromPointer" @keydown="escapeMenu" @focusout="leaveMenu">
    <div class="sidebar-session-main">
      <button class="workspace-session" :class="{ active: selected }" :aria-current="selected ? 'true' : undefined" :title="`${session.title}\n${workspaceName ? `${workspaceName} · ` : ''}${providerLabel(session.provider)} · ${t(session.status)}`" draggable="true" @dragstart="dragSession" @click="emit('open', session)">
        <span :class="['workspace-session-provider', session.provider]"><ProviderIcon :provider="session.provider" :size="14" /></span>
        <span class="workspace-session-copy"><strong>{{ session.title }}</strong><small><span :class="['state-dot', session.status]" />{{ t(session.status) }}<span>· {{ workspaceName || lastActive }}</span><Icon v-if="isSessionArchived(session)" name="archive" :size="10" :aria-label="t('Archived session')" /><span v-if="isEphemeralSession(session)" class="session-temporary-badge">{{ t('Temporary') }}</span></small></span>
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
    <div v-else-if="menuOpen" class="sidebar-session-menu">
      <button class="session-locate-action" @click="closeMenu(); emit('locate', session)"><Icon name="locate" :size="13" />{{ t('Locate in canvas') }}</button>
      <button class="session-rename-action" @click="beginRename"><Icon name="edit" :size="13" />{{ t('Rename session') }}</button>
      <button v-if="isEphemeralSession(session)" class="session-keep-action" :disabled="keepBusy" :aria-busy="keepBusy" :title="t('Keep this temporary session permanently. It stays in the session list and closing its window no longer discards it.')" @click="keepSession"><Icon name="check" :size="13" />{{ t(keepBusy ? 'Saving…' : 'Keep this session') }}</button>
      <button class="session-environment-action" @click="environment"><Icon name="settings" :size="13" />{{ t('Session environment') }}</button>
      <button class="session-archive-action" :disabled="!archiveSupported || archiveBusy" :aria-busy="archiveBusy" :title="!archiveSupported ? t('Upgrade the backend to archive sessions.') : t(isSessionArchived(session) ? 'Restore this session to its workspace list.' : 'Archive hides this session from workspace lists without stopping it.')" @click="archiveSession"><Icon :name="isSessionArchived(session) ? 'restore' : 'archive'" :size="13" />{{ t(archiveBusy ? 'Saving…' : isSessionArchived(session) ? 'Restore session' : 'Archive session') }}</button>
      <small v-if="!archiveSupported">{{ t('Upgrade the backend to archive sessions.') }}</small>
    </div>
  </li>
</template>

<style scoped>
.sidebar-session-row{min-width:0;list-style:none;scroll-margin-block:14px;border-radius:6px}.sidebar-session-main{display:flex;align-items:center;min-width:0}.workspace-session{display:flex;align-items:center;gap:7px;flex:1;min-width:0;border:1px solid transparent;border-radius:6px;padding:6px 4px;margin:1px 0;background:none;text-align:left}.workspace-session:hover{background:#f0f3f5}.workspace-session.active{background:#f0edf8;border-color:#e1daef}.workspace-session-provider{display:grid;place-items:center;flex-shrink:0;width:21px;height:24px;border-radius:5px;color:#b48663;background:#f8f0e9}.workspace-session-provider.codex{color:#8973b4;background:#efeaf8}.workspace-session-provider.terminal{color:#849ba7;background:#eef3f5}.workspace-session-copy{min-width:0;flex:1}.workspace-session-copy>strong{display:block;font-size:9px;font-weight:550;color:#687f89;line-height:15px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.workspace-session.active strong{color:#795d9b}.workspace-session-copy>small{display:flex;align-items:center;gap:4px;font-size:8px;line-height:14px;color:#899da7;white-space:nowrap}.workspace-session-copy>small>span:nth-child(2){overflow:hidden;text-overflow:ellipsis}.workspace-session-copy>small>svg{flex:none;color:#ad8d60}.sidebar-session-actions{display:flex;align-items:center;flex:none;gap:1px}.sidebar-session-actions>.icon-button{width:23px;height:28px;padding:4px}.session-locate-action>svg{color:#688fb9}.session-more-button{font-size:17px;line-height:18px;color:#897aa3}.session-more-button:hover,.session-more-button.selected{color:#75608f;background:#f0edf7}.sidebar-session-menu{margin:0 2px 6px 27px;padding:4px;border:1px solid #e1e9e7;border-radius:6px;background:#fff}.sidebar-session-menu>button{display:flex;align-items:center;gap:6px;width:100%;border:0;border-radius:4px;background:none;padding:6px 5px;text-align:left;font-size:9px;line-height:15px;color:#637c86}.sidebar-session-menu>button>svg{flex:none}.sidebar-session-menu>button:hover:not(:disabled){background:#edf6f2}.sidebar-session-menu>button:disabled{opacity:.55;cursor:not-allowed}.session-temporary-badge{flex:none;border-radius:4px;padding:0 4px;background:#efeaf8;color:#8973b4;font-size:8px;line-height:13px}.session-keep-action>svg{color:#8973b4}.session-environment-action>svg{color:#7984af}.session-archive-action>svg{color:#ad8d60}.archived .session-archive-action>svg{color:#448d76}.sidebar-session-menu>small{display:block;padding:2px 5px;font-size:8px;line-height:14px;color:#9c875f}
.session-rename-action>svg{color:#688fb9}.sidebar-session-rename{display:flex;align-items:center;gap:4px;margin:2px 2px 6px 27px;padding:3px;border:1px solid #b9dcd3;border-radius:7px;background:#f8fcfb}.sidebar-session-rename input{min-width:0;flex:1;border:0;background:transparent;padding:5px 4px;font-size:11px;color:#506b75;outline:0}.sidebar-session-rename button{display:grid;place-items:center;width:26px;height:26px;border:0;border-radius:5px;background:none;color:#6e858c;cursor:pointer}.sidebar-session-rename button:first-of-type{color:#287d6e}.sidebar-session-rename button:hover{background:#e5f3ee}
@media(pointer:coarse){.workspace-session{min-height:44px}.sidebar-session-actions>.icon-button{width:34px;height:44px}.sidebar-session-menu>button{min-height:44px;font-size:12px}.sidebar-session-rename{min-height:44px}.sidebar-session-rename input{font-size:16px}.sidebar-session-rename button{width:36px;height:36px}.workspace-session-copy>strong{font-size:11px}.workspace-session-copy>small{font-size:10px}}
</style>
