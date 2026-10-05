<script setup lang="ts">
/**
 * One session's choice about AgentDock's agent tools (server agent.rs):
 * follow the preference for every session, or on, or off for this one. The
 * tools are injected at launch, so a change applies at the next start.
 */
import { onMounted, ref } from "vue";
import { errorMessage, json, request } from "./api";
import { useI18n } from "../i18n";

interface State { choice: boolean | null; default: boolean; enabled: boolean }
const props = defineProps<{ sessionId: string }>();
const { t } = useI18n();
const state = ref<State>(), busy = ref(false), error = ref("");
const path = () => `/sessions/${encodeURIComponent(props.sessionId)}/agent-tools`;

onMounted(async () => { try { state.value = await request<State>(path()); } catch { /* An older server: nothing to show. */ } });
async function choose(choice: boolean | null) {
  if (busy.value || state.value?.choice === choice) return;
  busy.value = true; error.value = "";
  try { state.value = await request<State>(path(), json("PUT", { choice })); }
  catch (cause) { error.value = errorMessage(cause); }
  finally { busy.value = false; }
}
</script>

<template>
  <div v-if="state" class="agent-tools-switch" role="group" :aria-label="t('AgentDock tools')">
    <span class="agent-tools-label">{{ t('AgentDock tools') }}</span>
    <div class="agent-tools-choices">
      <button type="button" :class="{ active: state.choice === null }" :aria-pressed="state.choice === null" :disabled="busy" @click.stop="choose(null)">{{ t(state.default ? 'Default · on' : 'Default · off') }}</button>
      <button type="button" :class="{ active: state.choice === true }" :aria-pressed="state.choice === true" :disabled="busy" @click.stop="choose(true)">{{ t('On') }}</button>
      <button type="button" :class="{ active: state.choice === false }" :aria-pressed="state.choice === false" :disabled="busy" @click.stop="choose(false)">{{ t('Off') }}</button>
    </div>
    <small>{{ error || t('Takes effect when this session next starts.') }}</small>
  </div>
</template>

<style scoped>
.agent-tools-switch{display:flex;flex-direction:column;gap:6px;padding:8px 10px;border-top:1px solid var(--border);margin-top:4px}
.agent-tools-label{font-size:var(--text-xs);font-weight:600;color:var(--ink)}
.agent-tools-choices{display:flex;gap:2px;padding:2px;border-radius:var(--radius-sm);background:var(--fill)}
.agent-tools-choices>button{flex:1;min-height:24px;padding:2px 6px;border:0;border-radius:var(--radius-sm);background:none;font:inherit;font-size:var(--text-xs);color:var(--ink-soft);cursor:pointer;white-space:nowrap}
.agent-tools-choices>button.active{background:var(--surface);color:var(--teal);font-weight:600;box-shadow:var(--shadow-sm)}
.agent-tools-switch>small{font-size:var(--text-xs);line-height:1.4;color:var(--muted)}
@media (pointer:coarse){.agent-tools-choices>button{min-height:38px;font-size:var(--text-md)}.agent-tools-label{font-size:var(--text-md)}.agent-tools-switch>small{font-size:var(--text-sm)}}
</style>
