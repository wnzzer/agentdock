<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { EndpointProfile, ProviderKind, Session } from "@agentdock/protocol";
import ChatSessionPane from "./ChatSessionPane.vue";
import NativeSessionPane from "./NativeSessionPane.vue";
import ModalDialog from "./ModalDialog.vue";
import Icon from "./Icon.vue";
import { useI18n } from "../i18n";
import { sessionView, setSessionView, type SessionView } from "./session-view";
import { isEphemeralSession } from "./session-list";

const props = defineProps<{
  paneId?: string;
  session: Session;
  sessions: Session[];
  profiles: EndpointProfile[];
}>();

const emit = defineEmits<{
  changed: [];
  renameRequest: [id: string];
  environment: [id: string];
  profiles: [];
  create: [provider?: ProviderKind];
  select: [session: Session];
  reveal: [id: string];
  keep: [id: string];
}>();

const { t } = useI18n();
const view = ref<SessionView>(sessionView(props.session));
const showInfo = ref(false);
const isTerminal = computed(() => props.session.provider === "terminal");
const isChat = computed(() => !isTerminal.value && view.value === "chat");
// An escape hatch has no chat view of its own: it reopens a conversation that
// is already on screen in the structured session it came from, so offering one
// here would show the same transcript twice under two different session ids.
const isReopen = computed(() => !!props.session.resume_source_id);
const canSwitch = computed(() => !isTerminal.value && !isReopen.value && props.session.interaction_mode !== "structured");
const isEphemeral = computed(() => isEphemeralSession(props.session));

watch(
  () => [props.session.id, props.session.provider, props.session.interaction_mode] as const,
  () => { view.value = sessionView(props.session); },
  { immediate: true },
);
watch(() => props.session.id, () => { showInfo.value = false; });

function choose(next: SessionView): void {
  if (!canSwitch.value || next === view.value) return;
  view.value = setSessionView(props.session, next);
}
function openNative(): void { choose("native"); }
function openChat(): void {
  // This only changes the view. It deliberately does not call /start or
  // enable structured mode; the native process and its session id are kept.
  choose("chat");
}
function onStructured(id: string): void {
  if (id === props.session.id) openChat();
}
function showRunInfo(closeMenu: () => void): void {
  closeMenu();
  showInfo.value = true;
}
function renameSession(id: string): void { emit("renameRequest", id); }
</script>

<template>
  <section class="session-shell" :data-session-id="session.id" :data-view="view" :aria-label="t('Session view for {session}', { session: session.title })">
    <div class="session-shell-content" :data-session-id="session.id">
      <ChatSessionPane
        v-if="isChat"
        :key="`chat:${session.id}`"
        :pane-id="paneId"
        :session="session"
        :profiles="profiles"
        :consume-open-intent="session.interaction_mode === 'structured'"
        @changed="emit('changed')"
        @rename-request="renameSession"
        @environment="emit('environment', $event)"
        @profiles="emit('profiles')"
        @legacy="openNative"
        @open-session="emit('select', $event)"
      >
        <template #session-actions="{ closeMenu }">
          <button type="button" class="session-shell-action" @click="closeMenu(); renameSession(session.id)"><Icon name="edit" :size="14" />{{ t('Rename session') }}</button>
          <button v-if="isEphemeral" type="button" class="session-shell-action session-keep-action" @click="closeMenu(); emit('keep', session.id)"><Icon name="check" :size="14" />{{ t('Keep this session') }}</button>
          <button type="button" class="session-shell-action" @click="showRunInfo(closeMenu)">{{ t('Run information') }}</button>
        </template>
      </ChatSessionPane>
      <NativeSessionPane
        v-else
        :key="`native:${session.id}`"
        :pane-id="paneId"
        :session="session"
        :sessions="sessions"
        :profiles="profiles"
        :terminal="isTerminal"
        :consume-open-intent="false"
        @changed="emit('changed')"
        @rename-request="renameSession"
        @create="emit('create', $event)"
        @select="emit('select', $event)"
        @environment="emit('environment', $event)"
        @structured="onStructured"
      >
        <template #session-actions="{ closeMenu }">
          <button type="button" class="session-shell-action" @click="closeMenu(); renameSession(session.id)"><Icon name="edit" :size="14" />{{ t('Rename session') }}</button>
          <button v-if="isEphemeral" type="button" class="session-shell-action session-keep-action" @click="closeMenu(); emit('keep', session.id)"><Icon name="check" :size="14" />{{ t('Keep this session') }}</button>
          <button v-if="canSwitch" type="button" class="session-shell-action" @click="closeMenu(); openChat()">{{ t('Chat') }}</button>
          <button type="button" class="session-shell-action" @click="showRunInfo(closeMenu)">{{ t('Run information') }}</button>
        </template>
      </NativeSessionPane>
    </div>
    <ModalDialog v-if="showInfo" :title="t('Run information')" @close="showInfo = false">
      <dl class="session-runtime-info" :data-session-id="session.id">
        <div><dt>{{ t('Session ID') }}</dt><dd><code>{{ session.id }}</code></dd></div>
        <div><dt>{{ t('Provider session ID') }}</dt><dd><code>{{ session.provider_session_id || '—' }}</code></dd></div>
        <div><dt>{{ t('Mode') }}</dt><dd>{{ session.interaction_mode || 'pty' }}</dd></div>
        <div v-if="isEphemeral"><dt>{{ t('Temporary window') }}</dt><dd>{{ t('Discarded when its window is closed') }}</dd></div>
        <div><dt>{{ t('Workspace') }}</dt><dd><code>{{ session.workspace_id }}</code></dd></div>
        <div><dt>{{ t('Configuration revision') }}</dt><dd>{{ session.configuration_revision ?? 0 }}</dd></div>
      </dl>
    </ModalDialog>
  </section>
</template>

<style scoped>
.session-shell{display:flex;flex:1;min-height:0;min-width:0;flex-direction:column;overflow:hidden;background:var(--surface,#fff)}
.session-shell-content{display:flex;flex:1;min-height:0;min-width:0}.session-shell-content>*{flex:1 1 auto;width:auto;min-width:0;min-height:0}
.session-shell-action>svg{margin-right:7px}.session-keep-action>svg{color:#8973b4}
.session-shell-action{display:block;width:100%;min-height:44px;padding:9px;border:0;border-radius:7px;background:none;text-align:left;font:inherit;font-size:12px;color:#647681;cursor:pointer}.session-shell-action:hover{background:var(--teal-soft,#edf6f3)}.session-shell-action:focus-visible{outline:2px solid #51b4a3;outline-offset:2px}
.session-runtime-info{display:grid;gap:16px;margin:0;color:var(--text,#273745);font-size:13px}.session-runtime-info>div{display:grid;grid-template-columns:minmax(100px,1fr) minmax(0,2fr);align-items:start;gap:8px 16px}.session-runtime-info dt{color:#647681;font-size:12px}.session-runtime-info dd{margin:0;min-width:0;overflow-wrap:anywhere}.session-runtime-info code{font:12px/1.6 ui-monospace,monospace}
@media(max-width:520px){.session-runtime-info>div{grid-template-columns:1fr;gap:5px}}
</style>
