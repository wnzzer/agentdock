<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { setThemePreference, themePreference, type ThemePreference } from './theme';
import { desktopNotifications, notificationSupport, setDesktopNotifications } from './attention';
import type { EndpointProfile } from '@agentdock/protocol';
import ProviderIcon from './ProviderIcon.vue';
import Icon from './Icon.vue';
import { errorMessage, json, providerLabel, request } from './api';
import { REASONING_EFFORTS, effortLabel } from './reasoning-effort';
import { PERMISSION_CHOICES, loadPreferences, preferences, savePreferences, type PreferenceProvider, type Preferences } from './preferences';
import { backendCapabilities } from './backend-capabilities';
import { useI18n } from '../i18n';

/**
 * Defaults for new sessions: which account, how deeply to think, and how
 * tools are approved. Each takes effect when a session is created or
 * started; the dialog and the session's own chips can still change it.
 * A change saves at once -- there is nothing to lose by not asking.
 */
const props = defineProps<{ profiles: EndpointProfile[] }>();
const { t, locale, setLocale } = useI18n();
const PROVIDERS: PreferenceProvider[] = ['claude_code', 'codex'];
const PERMISSION_LABELS: Record<string, string> = { ask: 'Ask every time', plan: 'Plan first', accept_edits: 'Accept edits', danger: 'Never ask' };
const loaded = ref(false), saving = ref(false), error = ref(''), saved = ref(false);
const profilesFor = (provider: PreferenceProvider) => props.profiles.filter(profile => profile.provider === provider);
/** A preference naming a deleted profile reads as none, and says so. */
const staleProfile = computed(() => Object.fromEntries(PROVIDERS.map(provider => {
  const id = preferences.value[provider].endpoint_profile_id;
  return [provider, !!id && !profilesFor(provider).some(profile => profile.id === id)];
})));

onMounted(async () => { await loadPreferences(true); loaded.value = true; });

async function update(provider: PreferenceProvider, field: 'endpoint_profile_id' | 'effort' | 'permission', value: string) {
  const next: Preferences = JSON.parse(JSON.stringify(preferences.value));
  if (value) next[provider][field] = value; else delete next[provider][field];
  saving.value = true; error.value = ''; saved.value = false;
  try { await savePreferences(next); saved.value = true; }
  catch (cause) { error.value = errorMessage(cause); }
  finally { saving.value = false; }
}
/** Off is stored as `false`; on is the default, so it is stored as nothing. */
async function setSwitch(key: 'agent_tools' | 'resource_monitoring', on: boolean) {
  const next: Preferences = JSON.parse(JSON.stringify(preferences.value));
  if (on) delete next[key]; else next[key] = false;
  saving.value = true; error.value = ''; saved.value = false;
  try { await savePreferences(next); saved.value = true; }
  catch (cause) { error.value = errorMessage(cause); }
  finally { saving.value = false; }
}
/** Clearing asks twice: the first press arms it for a few seconds. */
const clearArmed = ref(false), clearing = ref(false), cleared = ref(false);
let disarm: ReturnType<typeof setTimeout> | undefined;
async function clearHistory() {
  if (!clearArmed.value) {
    clearArmed.value = true; cleared.value = false;
    disarm = setTimeout(() => { clearArmed.value = false; }, 4000);
    return;
  }
  if (disarm) clearTimeout(disarm);
  clearArmed.value = false; clearing.value = true; error.value = '';
  try { await request('/host/resources/history', { method: 'DELETE' }); cleared.value = true; }
  catch (cause) { error.value = errorMessage(cause); }
  finally { clearing.value = false; }
}
const pick = (provider: PreferenceProvider, field: 'endpoint_profile_id' | 'effort' | 'permission') => (event: Event) => update(provider, field, (event.target as HTMLSelectElement).value);
/** Desktop notifications need a secure page and the browser's permission; the row says which is missing. */
const support = ref(notificationSupport());
const notificationsPossible = computed(() => support.value !== 'unsupported' && support.value !== 'insecure' && support.value !== 'denied');
const notificationNote = computed(() => ({
  unsupported: 'This browser cannot show notifications.',
  insecure: 'Browsers allow notifications only over https or on localhost.',
  denied: 'This browser blocks notifications for this page; allow them in its site settings.',
  default: 'When a session needs you while this page is in the background.',
  granted: 'When a session needs you while this page is in the background.',
}[support.value]));
async function toggleNotifications(on: boolean) { await setDesktopNotifications(on); support.value = notificationSupport(); }
/** Temporary read-only grants for the file preview, so they can be seen and ended. */
const hostGrants = ref<Array<{ path: string; directory: boolean }>>([]);
onMounted(async () => { try { hostGrants.value = await request('/host/grants'); } catch { hostGrants.value = []; } });
async function revokeGrants() { try { hostGrants.value = await request('/host/grants/all', json('DELETE')); } catch { /* Left as listed. */ } }
</script>

<template>
  <section class="preferences-page">
    <div class="settings-lead"><p>{{ t('What a new session starts with. The new-session dialog and each session\'s own controls can still change it.') }}</p><span v-if="saving||saved" class="preferences-state" role="status">{{ t(saving ? 'Saving…' : 'Saved') }}</span></div>
    <p v-if="error" class="account-error" role="alert">{{ error }}</p>
    <!-- The interface language belongs to this browser, not the server: it is
         a per-viewer choice, and a phone and a laptop may well differ. -->
    <article class="preference-card">
      <header><span class="preference-mark general"><Icon name="settings" :size="17"/></span><strong>{{ t('General') }}</strong></header>
      <label class="preference-row">
        <span><strong>{{ t('Language') }}</strong><small>{{ t('For this browser.') }}</small></span>
        <select :value="locale" :aria-label="t('Language')" @change="setLocale(($event.target as HTMLSelectElement).value === 'en' ? 'en' : 'zh-CN')"><option value="zh-CN" lang="zh-CN">中文</option><option value="en" lang="en">English</option></select>
      </label>
      <label class="preference-row">
        <span><strong>{{ t('Appearance') }}</strong><small>{{ t('For this browser.') }}</small></span>
        <select :value="themePreference" :aria-label="t('Appearance')" @change="setThemePreference(($event.target as HTMLSelectElement).value as ThemePreference)"><option value="system">{{ t('Match system') }}</option><option value="light">{{ t('Light') }}</option><option value="dark">{{ t('Dark') }}</option></select>
      </label>
      <label class="preference-row">
        <span><strong>{{ t('Desktop notifications') }}</strong><small>{{ t(notificationNote) }}</small></span>
        <select :value="desktopNotifications ? 'on' : 'off'" :disabled="!notificationsPossible" :aria-label="t('Desktop notifications')" @change="toggleNotifications(($event.target as HTMLSelectElement).value === 'on')"><option value="off">{{ t('Off') }}</option><option value="on">{{ t('On') }}</option></select>
      </label>
      <div v-if="hostGrants.length" class="preference-row">
        <span><strong>{{ t('Temporary file access') }}</strong><small>{{ t('Read-only, for the file preview, until AgentDock restarts.') }}</small><code v-for="item in hostGrants" :key="item.path" class="grant-path">{{ item.path }}{{ item.directory ? '/' : '' }}</code></span>
        <button type="button" class="small-button danger" @click="revokeGrants">{{ t('Revoke all') }}</button>
      </div>
    </article>
    <p v-if="!loaded" class="preferences-quiet">{{ t('Loading…') }}</p>
    <template v-else>
      <article v-for="provider in PROVIDERS" :key="provider" class="preference-card">
        <header><span :class="['preference-mark',provider]"><ProviderIcon :provider="provider" :size="17"/></span><strong>{{ providerLabel(provider) }}</strong></header>
        <label class="preference-row">
          <span><strong>{{ t('Default endpoint') }}</strong><small>{{ t('Chosen in the new-session dialog, and used by one-click sessions.') }}</small></span>
          <select :value="staleProfile[provider] ? '' : preferences[provider].endpoint_profile_id ?? ''" :disabled="saving" @change="pick(provider,'endpoint_profile_id')($event)">
            <option value="">{{ t('Last used') }}</option>
            <option v-for="profile in profilesFor(provider)" :key="profile.id" :value="profile.id">{{ profile.name }}</option>
          </select>
        </label>
        <p v-if="staleProfile[provider]" class="preference-note">{{ t('The profile chosen before no longer exists; the last used one applies.') }}</p>
        <label class="preference-row">
          <span><strong>{{ t('Default thinking depth') }}</strong><small>{{ t(provider === 'codex' ? 'Only where the model offers it.' : 'Deeper is slower and uses more of your quota.') }}</small></span>
          <select :value="preferences[provider].effort ?? ''" :disabled="saving" @change="pick(provider,'effort')($event)">
            <option value="">{{ t('Automatic · provider default') }}</option>
            <option v-for="level in REASONING_EFFORTS" :key="level" :value="level">{{ t(effortLabel(level)) }}</option>
          </select>
        </label>
        <label class="preference-row">
          <span><strong>{{ t('Default permission') }}</strong><small>{{ t('How tools are approved each time a session starts. The session\'s own chip can change it between turns.') }}</small></span>
          <select :value="preferences[provider].permission ?? ''" :class="{danger:preferences[provider].permission==='danger'}" :disabled="saving" @change="pick(provider,'permission')($event)">
            <option value="">{{ t('Client default') }}</option>
            <option v-for="mode in PERMISSION_CHOICES[provider]" :key="mode" :value="mode">{{ t(PERMISSION_LABELS[mode]) }}</option>
          </select>
        </label>
        <p v-if="preferences[provider].permission==='danger'" class="preference-note danger">{{ t('New sessions will run tools without asking. Only choose this for directories you trust completely.') }}</p>
      </article>
      <!-- What AgentDock itself does on this host, beside the clients' defaults. -->
      <article class="preference-card">
        <header><span class="preference-mark general"><Icon name="spark" :size="17"/></span><strong>AgentDock</strong></header>
        <label class="preference-row">
          <span><strong>{{ t('AgentDock tools for agents') }}</strong><small>{{ t('Claude Code and Codex sessions can look at AgentDock and, once you confirm here, change it: endpoints, defaults, workspaces. Takes effect when a session next starts.') }}</small></span>
          <input type="checkbox" class="preference-switch" role="switch" :checked="preferences.agent_tools !== false" :disabled="saving" :aria-label="t('AgentDock tools for agents')" @change="setSwitch('agent_tools', ($event.target as HTMLInputElement).checked)" />
        </label>
        <template v-if="backendCapabilities.resourceMonitoring">
        <label class="preference-row">
          <span><strong>{{ t('Resource monitoring') }}</strong><small>{{ t('Reads the host\'s CPU, memory, temperature, fans and clock for the status bar and the system page, and every five seconds keeps them, with each session\'s share, on this machine for up to 90 days. Off reads nothing at all; what is already kept stays until cleared.') }}</small></span>
          <input type="checkbox" class="preference-switch" role="switch" :checked="preferences.resource_monitoring !== false" :disabled="saving" :aria-label="t('Resource monitoring')" @change="setSwitch('resource_monitoring', ($event.target as HTMLInputElement).checked)" />
        </label>
        <div class="preference-row">
          <span><strong>{{ t('Recorded history') }}</strong><small>{{ cleared ? t('Cleared.') : t('Removes every recorded sample. Recording continues while monitoring is on.') }}</small></span>
          <button type="button" :class="['small-button', { danger: clearArmed }]" :disabled="clearing" @click="clearHistory">{{ t(clearArmed ? 'Press again to clear' : 'Clear history') }}</button>
        </div>
        </template>
      </article>
    </template>
  </section>
</template>

<style scoped>
.preferences-page{min-width:0}
.settings-lead{display:flex;align-items:center;gap:12px;margin:2px 0 16px}
.settings-lead p{flex:1;margin:0;font-size:var(--text-sm);line-height:1.7;color:var(--ink-soft)}
.preferences-state{flex-shrink:0;font-size:var(--text-xs);color:var(--muted)}
.preferences-quiet{font-size:var(--text-xs);color:var(--muted)}
.preference-card{border:1px solid var(--border);border-radius:var(--radius-lg);padding:13px 14px 6px;margin-bottom:10px;background:var(--surface)}
.preference-card header{display:flex;align-items:center;gap:10px;margin-bottom:6px}
.preference-card header strong{font-size:var(--text-md);font-weight:600;color:var(--ink)}
.preference-mark{display:grid;place-items:center;width:30px;height:30px;border-radius:var(--radius-md);background:var(--codex-soft);color:var(--codex);flex-shrink:0}
.preference-mark.claude_code{background:var(--claude-soft);color:var(--claude)}
/* Only the cards carry an icon; the rows are read by their labels. Functional
   icons take the text's tone rather than a hue of their own (icon-palette.ts). */
.preference-mark.general{background:var(--fill);color:var(--ink-soft)}
.preference-row{display:flex;align-items:center;gap:14px;padding:9px 0;border-top:1px solid var(--border)}
.preference-row.first{border-top:0;padding-top:0}
.preference-switch{appearance:none;flex:none;position:relative;width:38px;height:22px;margin:0;border-radius:var(--radius-md);background:var(--line-strong);cursor:pointer;transition:background .15s}
.preference-switch::after{content:"";position:absolute;top:2px;left:2px;width:18px;height:18px;border-radius:50%;background:var(--surface);box-shadow:var(--shadow-sm);transition:transform .15s}
.preference-switch:checked{background:var(--teal)}
.preference-switch:checked::after{transform:translateX(16px)}
.preference-switch:focus-visible{outline:2px solid var(--focus,var(--focus));outline-offset:2px}
.preference-row>span{flex:1;min-width:0}
.preference-row strong{display:block;font-size:var(--text-sm);font-weight:550;color:var(--ink)}
.preference-row small{display:block;margin-top:2px;font-size:var(--text-xs);line-height:1.5;color:var(--muted)}
.preference-row select{width:190px;flex-shrink:0;height:32px;padding:0 8px;border:1px solid var(--line);border-radius:var(--radius-md);background:var(--surface);font:inherit;font-size:var(--text-sm);color:var(--ink)}
.preference-row select.danger{color:var(--danger-ink);border-color:var(--danger-line)}
.preference-note{margin:-4px 0 8px;font-size:var(--text-xs);line-height:1.5;color:var(--muted)}
.preference-note.danger{color:var(--danger-ink)}
@media(max-width:520px){.preference-row{flex-direction:column;align-items:stretch;gap:6px}.preference-row select{width:100%;height:40px}}
.grant-path{display:block;margin-top:4px;font-family:var(--mono);font-size:var(--text-xs);color:var(--ink-soft);overflow-wrap:anywhere}
</style>
