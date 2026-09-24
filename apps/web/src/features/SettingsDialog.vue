<script setup lang="ts">
import { ref } from 'vue';
import type { EndpointProfile } from '@agentdock/protocol';
import ModalDialog from './ModalDialog.vue';
import Icon from './Icon.vue';
import AgentClientsPanel from './AgentClientsPanel.vue';
import ProfilesDialog from './ProfilesDialog.vue';
import AccountsDialog from './AccountsDialog.vue';
import PreferencesPanel from './PreferencesPanel.vue';
import { useI18n } from '../i18n';

/**
 * One settings entry with its pages inside. Agents, endpoints and accounts were
 * three separate top-level dialogs; they are one place now, because choosing
 * between them is part of configuring the same thing.
 */
type SettingsSection = 'preferences' | 'agents' | 'endpoints' | 'accounts';
const props = defineProps<{ profiles: EndpointProfile[]; initialSection?: SettingsSection }>();
const emit = defineEmits<{ close: []; changed: [profile?: EndpointProfile] }>();
const { t } = useI18n();

const SECTIONS = [
  { id: 'preferences', icon: 'gauge', label: 'Preferences' },
  { id: 'agents', icon: 'spark', label: 'Agent clients' },
  { id: 'endpoints', icon: 'settings', label: 'Endpoint profiles' },
  { id: 'accounts', icon: 'account', label: 'Official accounts' },
] as const;

const section = ref<SettingsSection>(props.initialSection ?? 'agents');
const profilesPage = ref<{ requestLeave: (action: () => void) => void }>();

/** The endpoints page guards unsaved edits, so leaving it goes through it. */
function leave(action: () => void) {
  if (section.value === 'endpoints' && profilesPage.value) profilesPage.value.requestLeave(action);
  else action();
}
function select(next: SettingsSection) { if (next !== section.value) leave(() => { section.value = next; }); }
</script>

<template>
  <ModalDialog :title="t('Settings')" wide @close="leave(() => emit('close'))">
    <div class="settings-layout">
      <nav class="settings-nav" :aria-label="t('Settings sections')">
        <button v-for="entry in SECTIONS" :key="entry.id" type="button" :class="['settings-nav-item',{selected:section===entry.id}]" :aria-current="section===entry.id?'page':undefined" @click="select(entry.id)">
          <Icon :name="entry.icon" :size="16"/>
          <strong>{{ t(entry.label) }}</strong>
        </button>
      </nav>
      <div class="settings-page">
        <PreferencesPanel v-if="section==='preferences'" :profiles="profiles"/>
        <AgentClientsPanel v-else-if="section==='agents'"/>
        <ProfilesDialog v-else-if="section==='endpoints'" ref="profilesPage" embedded :profiles="profiles" @close="emit('close')" @changed="emit('changed', $event)" @accounts="section='accounts'"/>
        <AccountsDialog v-else embedded @close="emit('close')" @changed="emit('changed', $event)"/>
      </div>
    </div>
  </ModalDialog>
</template>

<style scoped>
/* The dialog keeps one size across its pages, and only the page scrolls: a
   long form used to carry the whole dialog, navigation and list with it. */
.settings-layout{display:grid;grid-template-columns:168px minmax(0,1fr);gap:20px;height:min(640px,calc(100vh - 190px));min-height:420px}
.settings-nav{display:flex;flex-direction:column;gap:3px;border-right:1px solid var(--border);padding-right:14px}
.settings-nav-item{display:flex;align-items:center;gap:10px;width:100%;min-height:40px;text-align:left;padding:8px 10px;border:0;border-radius:9px;background:none;color:var(--ink-soft);cursor:pointer;font:inherit}
.settings-nav-item:hover{background:var(--fill)}
.settings-nav-item.selected{background:var(--teal-soft);color:var(--teal)}
.settings-nav-item strong{font-size:12.5px;font-weight:550;color:inherit}
.settings-nav-item:focus-visible{outline:2px solid var(--focus);outline-offset:2px}
.settings-page{min-width:0;min-height:0;overflow-y:auto;overflow-x:hidden;padding-right:4px}
@media(max-width:680px){
  .settings-layout{grid-template-columns:minmax(0,1fr);gap:14px;height:auto;min-height:0}
  .settings-nav{flex-direction:row;border-right:0;border-bottom:1px solid var(--border);padding:0 0 10px;overflow-x:auto}
  .settings-nav-item{width:auto;flex-shrink:0;min-height:44px}
  .settings-page{overflow:visible;padding-right:0}
}
</style>
