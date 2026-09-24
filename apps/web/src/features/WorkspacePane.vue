<script setup lang="ts">
import { computed } from "vue";
import type { EndpointProfile, GitStatus, PaneNode, ProviderKind, Session, Workspace } from "@agentdock/protocol";
import { paneString, paneSession, paneWorkspace } from "./pane-context";
import NativeSessionPane from "./NativeSessionPane.vue";
import SessionPane from "./SessionPane.vue";
import FilePane from "./FilePane.vue";
import GitPane from "./GitPane.vue";
import Icon from "./Icon.vue";
import { useI18n } from "../i18n";
const { t } = useI18n();
const props = defineProps<{ pane: PaneNode; workspaces: Workspace[]; sessions: Session[]; profiles: EndpointProfile[]; gitRefresh: Record<string, number> }>();
const emit = defineEmits<{
  sessionChanged: []; newSession: [id: string | undefined, provider?: ProviderKind]; openSession: [session: Session]; renameRequest: [id: string]; sessionEnvironment: [id: string];
  structuredSession: [id: string]; profiles: [];
  revealSession: [id: string]; keepSession: [id: string];
  gitChanged: [id: string, status: GitStatus]; openFile: [id: string, path: string]; openReference: [id: string, reference: { path: string; line?: number; checkout?: string | null }]; filesSaved: [id: string];
  browse: [id: string | undefined]; reveal: [id: string, path: string];
}>();
const workspace = computed(() => paneWorkspace(props.pane, props.workspaces, props.sessions));
const session = computed(() => paneSession(props.pane, props.sessions));
const invalid = computed(() => (!!paneString(props.pane, "workspace_id") && !workspace.value) || (!!paneString(props.pane, "session_id") && !session.value) || (!!paneString(props.pane, "path") && !workspace.value));
const choices = computed(() => workspace.value ? props.sessions.filter(session => session.workspace_id === workspace.value!.id) : props.sessions);
const isAgent = computed(() => props.pane.kind === "agent_chat" || props.pane.kind === "terminal");
const callbacks = computed(() => {
  const id = workspace.value?.id;
  return {
    newSession: (provider?: ProviderKind) => emit("newSession", id, provider),
    gitChanged: (status: GitStatus) => { if (id) emit("gitChanged", id, status); },
    openFile: (path: string) => { if (id) emit("openFile", id, path); },
    filesSaved: () => { if (id) emit("filesSaved", id); },
    browse: () => emit("browse", id),
    reveal: (path: string) => { if (id) emit("reveal", id, path); },
  };
});
function renameSession(id: string) { emit("renameRequest", id); }
</script>
<template>
  <section :class="['workspace-pane', { 'agent-pane': isAgent && !!session }]" :data-workspace-id="workspace?.id">
    <div class="pane-workspace-bar"><span :title="workspace?.root_path"><Icon name="folder" :size="12" />{{ workspace?.name || t('Unassigned workspace') }}</span><span class="pane-scope-label">{{ t('Fixed to this pane') }}</span></div>
    <div v-if="invalid" class="pane-empty"><Icon name="info" :size="26"/><h3>{{ t('Pane source unavailable') }}</h3><p>{{ t('This pane cannot resolve its original workspace or session. It will not use another workspace automatically.') }}</p></div>
    <SessionPane v-else-if="isAgent && session" :key="`session:${session.id}`" :pane-id="pane.id" :session="session" :sessions="choices" :profiles="profiles" @changed="emit('sessionChanged')" @create="callbacks.newSession" @select="emit('openSession',$event)" @rename-request="renameSession" @environment="emit('sessionEnvironment',$event)" @profiles="emit('profiles')" @reveal="emit('revealSession',$event)" @keep="emit('keepSession',$event)"  @open-file="(reference: { path: string; line?: number; checkout?: string | null }) => workspace && emit('openReference', workspace.id, reference)" />
    <NativeSessionPane v-else-if="isAgent" :key="`${pane.id}:${workspace?.id??''}`" :session="session" :sessions="choices" :profiles="profiles" :terminal="pane.kind==='terminal'" @changed="emit('sessionChanged')" @create="callbacks.newSession" @select="emit('openSession',$event)" @environment="emit('sessionEnvironment',$event)" @structured="emit('structuredSession',$event)" />
    <GitPane v-else-if="pane.kind==='git_diff' && workspace" :key="`${pane.id}:${workspace.id}`" :workspace-id="workspace.id" :refresh-token="gitRefresh[workspace.id]??0" @changed="callbacks.gitChanged" @open-file="callbacks.openFile" />
    <FilePane v-else-if="workspace" :key="`${pane.id}:${workspace.id}`" :workspace-id="workspace.id" :path="paneString(pane,'path')" @saved="callbacks.filesSaved" @browse="callbacks.browse" @reveal="callbacks.reveal" />
    <div v-else class="pane-empty"><Icon name="folder" :size="28"/><h3>{{ t('Choose a workspace source') }}</h3><p>{{ t('Open files or Git using a workspace shortcut in the sidebar.') }}</p></div>
  </section>
</template>
<style scoped>
.workspace-pane{display:flex;flex:1;min-height:0;min-width:0;flex-direction:column;height:100%;overflow:hidden}.pane-workspace-bar{display:flex;align-items:center;justify-content:space-between;gap:8px;flex:none;min-height:25px;padding:4px 10px;border-bottom:1px solid #e7eeeb;background:#f7faf9;color:#62877d;font-size:10px}.pane-workspace-bar>span:first-child{display:flex;align-items:center;gap:5px;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.pane-scope-label{font-size:9px;color:#98aaa3;white-space:nowrap}.workspace-pane.agent-pane>.pane-workspace-bar{display:none}@media(max-width:650px){.pane-scope-label{display:none}}
</style>
