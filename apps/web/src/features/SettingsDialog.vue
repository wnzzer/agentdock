<script setup lang="ts">
import { computed, ref } from 'vue';
import type { EndpointProfile } from '@agentdock/protocol';
import ModalDialog from './ModalDialog.vue';
import Icon from './Icon.vue';
import AgentClientsPanel from './AgentClientsPanel.vue';
import ProfilesDialog from './ProfilesDialog.vue';
import AccountsDialog from './AccountsDialog.vue';
import { useI18n } from '../i18n';

/**
 * One settings entry with its pages inside. Agents, endpoints and accounts were
 * three separate top-level dialogs; they are one place now, because choosing
 * between them is part of configuring the same thing.
 */
type SettingsSection = 'agents' | 'endpoints' | 'accounts';
const props = defineProps<{ profiles: EndpointProfile[]; initialSection?: SettingsSection }>();
const emit = defineEmits<{ close: []; changed: [profile?: EndpointProfile] }>();
const { t } = useI18n();

const SECTIONS = [
  { id: 'agents', icon: 'spark', label: 'Agent clients', hint: 'Installed clients and versions' },
  { id: 'endpoints', icon: 'settings', label: 'Endpoint profiles', hint: 'Models, proxies and permissions' },
  { id: 'accounts', icon: 'account', label: 'Official accounts', hint: 'Sign-in, usage and configurations' },
] as const;

const section = ref<SettingsSection>(props.initialSection ?? 'agents');
const profilesPage = ref<{ requestLeave: (action: () => void) => void }>();
const current = computed(() => SECTIONS.find(entry => entry.id === section.value));

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
          <span><strong>{{ t(entry.label) }}</strong><small>{{ t(entry.hint) }}</small></span>
        </button>
      </nav>
      <div class="settings-page">
        <h3 class="settings-page-title">{{ t(current?.label ?? 'Settings') }}</h3>
        <AgentClientsPanel v-if="section==='agents'"/>
        <ProfilesDialog v-else-if="section==='endpoints'" ref="profilesPage" embedded :profiles="profiles" @close="emit('close')" @changed="emit('changed', $event)"/>
        <AccountsDialog v-else embedded @close="emit('close')" @changed="emit('changed', $event)"/>
      </div>
    </div>
  </ModalDialog>
</template>

<style scoped>
.settings-layout{display:grid;grid-template-columns:212px minmax(0,1fr);gap:22px;min-height:440px}
.settings-nav{border-right:1px solid var(--border);padding-right:15px}
.settings-nav-item{display:flex;align-items:center;gap:10px;width:100%;min-height:56px;text-align:left;padding:10px;margin-bottom:5px;border:1px solid transparent;border-radius:10px;background:none;color:#647681;cursor:pointer}
.settings-nav-item.selected{border-color:#dbeee4;background:var(--teal-soft)}
.settings-nav-item span{min-width:0;flex:1}
.settings-nav-item strong{display:block;font-size:12px;font-weight:550;color:#273745}
.settings-nav-item small{display:block;margin-top:4px;font-size:10px;color:var(--muted);line-height:1.5}
.settings-nav-item:focus-visible{outline:2px solid #51b4a3;outline-offset:3px}
.settings-page{min-width:0}
.settings-page-title{margin:0 0 16px;font-size:11px;letter-spacing:.9px;font-weight:650;color:#95a2aa;text-transform:uppercase}
@media(max-width:680px){
  .settings-layout{grid-template-columns:minmax(0,1fr);gap:14px;min-height:0}
  .settings-nav{border-right:0;border-bottom:1px solid var(--border);padding-right:0;padding-bottom:11px;display:flex;gap:8px;overflow-x:auto}
  .settings-nav-item{width:auto;min-width:132px;flex-shrink:0;margin-bottom:0;min-height:52px}
  .settings-nav-item small{display:none}
  .settings-page-title{display:none}
}
</style>
