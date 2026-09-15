<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import type { Session, Workspace } from "@agentdock/protocol";
import { errorMessage, providerLabel, request } from "./api";
import ModalDialog from "./ModalDialog.vue";
import { fuzzyFilter } from "./fuzzy-search";
import Icon from "./Icon.vue";
import ProviderIcon from "./ProviderIcon.vue";
import { useI18n } from "../i18n";
const { t } = useI18n();
const props = defineProps<{ workspaces: Workspace[] }>();
const emit = defineEmits<{ close: []; open: [session: Session] }>();
const sessions = ref<Session[]>([]), query = ref(""), filter = ref("all"), error = ref(""), loading = ref(false);
const workspaceName = (id: string) => props.workspaces.find(workspace => workspace.id === id)?.name ?? t("Unknown workspace");
const visible = computed(() => fuzzyFilter(sessions.value.filter(session => filter.value === "all" || (filter.value === "active" ? ["running","starting","waiting"].includes(session.status) : session.status === filter.value)),query.value,session=>`${session.title} ${session.id} ${providerLabel(session.provider)} ${workspaceName(session.workspace_id)}`));
async function refresh() { loading.value = true; error.value = ""; try { sessions.value = await request<Session[]>("/sessions"); } catch (cause) { error.value = errorMessage(cause); } finally { loading.value = false; } }
onMounted(refresh);
</script>
<template><ModalDialog :title="t('All sessions')" wide @close="emit('close')"><p class="form-description">{{ t('A global view of sessions registered by AgentDock. Each session still belongs to its workspace.') }}</p><div class="global-session-filters"><div class="file-search"><Icon name="search" :size="15" /><input v-model="query" :aria-label="t('Search all sessions')" :placeholder="t('Search sessions, providers, workspaces…')" /></div><select v-model="filter" :aria-label="t('Filter sessions by status')"><option value="all">{{ t('All statuses') }}</option><option value="active">{{ t('Active') }}</option><option value="waiting">{{ t('Waiting') }}</option><option value="stopped">{{ t('Stopped') }}</option><option value="failed">{{ t('Failed') }}</option></select><button class="icon-button" :aria-label="t('Refresh all sessions')" :disabled="loading" @click="refresh"><Icon name="refresh" :size="16" /></button></div><div v-if="error" class="inline-error" role="alert">{{ error }}</div><div class="global-sessions"><button v-for="session in visible" :key="session.id" class="global-session-row" @click="emit('open', session)"><span :class="['provider-mark', session.provider]"><ProviderIcon :provider="session.provider" :size="18" /></span><span class="global-session-main"><strong>{{ session.title }}</strong><small>{{ workspaceName(session.workspace_id) }} · {{ session.provider === 'terminal' ? t('Terminal') : providerLabel(session.provider) }}</small></span><span :class="['status-pill', session.status]">{{ t(session.status) }}</span><Icon name="arrow" :size="16" /></button><div v-if="!visible.length" class="pane-empty"><Icon name="clock" :size="28" /><h3>{{ t(loading ? 'Loading sessions…' : 'No matching sessions') }}</h3><p>{{ t('Create a session inside a workspace to get started.') }}</p></div></div></ModalDialog></template>
