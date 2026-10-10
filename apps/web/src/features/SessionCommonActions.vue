<script setup lang="ts">
import type { Session } from "@agentdock/protocol";
import { useI18n } from "../i18n";
import { isEphemeralSession, resumeCommand } from "./session-list";
import { clientLabel } from "./clients";
import Icon from "./Icon.vue";
import CopyButton from "./CopyButton.vue";
const props = defineProps<{ session: Session; keepBusy?: boolean; closeMenu?: () => void }>();
const emit = defineEmits<{ rename: []; environment: []; keep: []; info: [] }>();
const { t } = useI18n();
function showRunInfo() { props.closeMenu?.(); emit("info"); }
function act(action: "rename" | "environment" | "keep") {
  props.closeMenu?.();
  if (action === "rename") emit("rename");
  else if (action === "environment") emit("environment");
  else emit("keep");
}
</script>
<template>
  <button class="session-rename-action" role="menuitem" @click="act('rename')"><Icon name="edit" :size="14" />{{ t('Rename session') }}</button>
  <button class="session-environment-action" role="menuitem" @click="act('environment')"><Icon name="settings" :size="14" />{{ t('Session environment') }}</button>
  <button v-if="isEphemeralSession(session)" class="session-keep-action" role="menuitem" :disabled="keepBusy" :aria-busy="keepBusy" @click="act('keep')"><Icon name="check" :size="14" />{{ t(keepBusy ? 'Saving…' : 'Keep this session') }}</button>
  <CopyButton v-if="resumeCommand(session)" :text="resumeCommand(session)!" label="Copy resume command" menu-item />
  <CopyButton v-if="session.provider_session_id && session.provider !== 'terminal'" :text="session.provider_session_id" :label="t('Copy {client} session ID', { client: clientLabel(session.provider) })" menu-item />
  <button role="menuitem" @click="showRunInfo"><Icon name="info" :size="14" />{{ t('Run information') }}</button>
</template>

<style scoped>
button{display:flex;align-items:center;gap:8px;width:100%;min-height:30px;padding:6px 9px;border:0;border-radius:var(--radius-sm);background:none;text-align:left;font:inherit;font-size:var(--text-sm);color:var(--ink-soft);cursor:pointer}
button:hover:not(:disabled){background:var(--fill)}button:focus-visible{outline:2px solid var(--focus);outline-offset:1px}button:disabled{opacity:.5}
@media(pointer:coarse){button{min-height:44px}}
</style>
