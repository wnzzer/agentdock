<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import type { AgentProviderKind } from '@agentdock/protocol';
import { backendCapabilities } from './backend-capabilities';
import { ApiError, errorMessage, json, providerLabel, request } from './api';
import Icon from './Icon.vue';
import ProviderIcon from './ProviderIcon.vue';
import { useI18n } from '../i18n';

/** Where the program a session launches was resolved from. */
type ProgramSource = 'override' | 'path' | 'managed' | 'missing';
interface ClientView {
  provider: AgentProviderKind;
  command: string;
  npm_package: string;
  program: string;
  source: ProgramSource;
  installed: boolean;
  version?: string | null;
  managed_path?: string | null;
  install_available: boolean;
  shadowed: boolean;
}

const { t } = useI18n();
const supported = computed(() => (backendCapabilities as typeof backendCapabilities & { clients?: boolean }).clients === true);
const clients = ref<ClientView[]>([]), loading = ref(false), busy = ref(''), error = ref(''), notice = ref(''), unavailable = ref(false);
const confirming = ref<AgentProviderKind>();
let disposed = false, revision = 0, controller: AbortController | undefined;

async function load() {
  if (!supported.value) return;
  const own = ++revision; controller?.abort(); controller = new AbortController(); loading.value = true; error.value = '';
  try {
    const items = await request<ClientView[]>('/clients', { signal: controller.signal });
    if (disposed || own !== revision) return;
    clients.value = items; unavailable.value = false;
  } catch (cause) {
    if (disposed || own !== revision || (cause instanceof Error && cause.name === 'AbortError')) return;
    unavailable.value = cause instanceof ApiError && [404, 501].includes(cause.status);
    error.value = unavailable.value ? '' : errorMessage(cause);
  } finally { if (own === revision) loading.value = false; }
}
async function install(client: ClientView) {
  if (busy.value || !client.install_available || confirming.value !== client.provider) return;
  busy.value = client.provider; error.value = ''; notice.value = '';
  try {
    const updated = await request<ClientView>('/clients/' + encodeURIComponent(client.provider) + '/install', json('POST', { confirmed: true }));
    if (disposed) return;
    clients.value = clients.value.map(item => item.provider === updated.provider ? updated : item);
    confirming.value = undefined;
    // An install can succeed and still change nothing a session will run.
    notice.value = updated.shadowed
      ? 'Installed, but sessions keep using the copy already on the host PATH.'
      : updated.installed ? 'Installed. New sessions will use this client.' : 'The install finished but the client still does not report a version.';
  } catch (cause) { if (!disposed) error.value = errorMessage(cause); }
  finally { if (!disposed) busy.value = ''; }
}
/** Claude reports "2.1.280 (Claude Code)" and Codex "codex-cli 0.154.0"; the
 * card already says which client it is, so only the number is kept. */
const versionLabel = (client: ClientView) => client.version?.replace(/\s*\([^)]*\)\s*$/, '').replace(/^[a-z-]+\s+(?=\d)/i, '') || t('No version reported');
function sourceLabel(client: ClientView) {
  return client.source === 'override' ? t('Server override')
    : client.source === 'path' ? t('Found on host PATH')
    : client.source === 'managed' ? t('Installed by AgentDock')
    : t('Not found');
}
onMounted(() => { void load(); });
onBeforeUnmount(() => { disposed = true; controller?.abort(); });
</script>

<template>
  <section class="clients-page">
    <div class="settings-lead"><p>{{ t('Sessions run the official clients installed on this host. A missing one can be installed from npm.') }}</p><button v-if="supported&&!unavailable" type="button" class="settings-lead-action" :disabled="!!busy||loading" :aria-label="t('Re-check')" :title="t('Re-check')" @click="load"><Icon name="refresh" :size="14"/></button></div>

    <div v-if="!supported||unavailable" class="clients-unavailable"><Icon name="settings" :size="25"/><h3>{{ t('Client management needs an updated backend') }}</h3><p>{{ t('Sessions still launch whatever client is already on this host. Nothing is installed or changed here.') }}</p></div>
    <template v-else>
      <p v-if="error" class="account-error" role="alert">{{ error }}</p>
      <p v-if="notice" class="account-notice" role="status">{{ notice }}</p>
      <p v-if="loading&&!clients.length" class="clients-quiet">{{ t('Checking this host…') }}</p>

      <article v-for="client in clients" :key="client.provider" class="client-card">
        <header>
          <span :class="['client-mark',client.provider]"><ProviderIcon :provider="client.provider" :size="19"/></span>
          <div>
            <strong>{{ providerLabel(client.provider) }} <em>{{ versionLabel(client) }}</em></strong>
            <small :title="client.program"><code>{{ client.program }}</code> · {{ sourceLabel(client) }}</small>
          </div>
          <span :class="['client-state',client.installed?'installed':'missing']">{{ t(client.installed?'Installed':'Not installed') }}</span>
        </header>
        <p v-if="client.managed_path&&client.shadowed" class="clients-quiet">{{ t('AgentDock has its own copy, but the host PATH copy wins. Installing again will not change which client sessions run.') }}</p>
        <div v-if="confirming===client.provider" class="account-confirm">
          <strong>{{ t('Install {name} from npm?', { name: providerLabel(client.provider) }) }}</strong>
          <p>{{ t('This downloads {package} from the npm registry and runs its install scripts on this host. It installs into AgentDock\'s own directory, not globally, and never replaces a client already on PATH.', { package: client.npm_package }) }}</p>
          <div class="account-buttons"><button :disabled="!!busy" @click="confirming=undefined">{{ t('Cancel') }}</button><button class="account-primary" :disabled="!!busy" @click="install(client)">{{ t(busy===client.provider?'Installing…':'Install now') }}</button></div>
        </div>
        <div v-else-if="client.install_available" class="client-actions">
          <button type="button" class="client-link" :disabled="!!busy" @click="confirming=client.provider;notice=''">{{ t(client.installed?'Install or update AgentDock copy…':'Install from npm…') }}</button>
        </div>
      </article>
    </template>
  </section>
</template>

<style scoped>
.clients-page{min-width:0}
.settings-lead{display:flex;align-items:center;gap:12px;margin:2px 0 16px}
.settings-lead p{flex:1;margin:0;font-size:12px;line-height:1.7;color:var(--ink-soft)}
.settings-lead-action{display:grid;place-items:center;width:30px;height:30px;flex-shrink:0;border:1px solid var(--border);border-radius:8px;background:var(--surface);color:var(--ink-soft);cursor:pointer}
.settings-lead-action:hover:not(:disabled){color:var(--teal);border-color:var(--teal-line)}
.clients-quiet{font-size:11px;line-height:1.7;color:var(--muted);margin:8px 0 0}
.client-card{border:1px solid var(--border);border-radius:12px;padding:13px 14px;margin-bottom:10px;background:var(--surface)}
.client-card header{display:flex;align-items:center;gap:11px}
.client-card header>div{flex:1;min-width:0}
.client-card strong{display:block;font-size:13px;font-weight:600;color:var(--ink)}
.client-card strong em{font-style:normal;font-weight:450;font-size:11px;color:var(--muted);margin-left:4px}
.client-card small{display:block;margin-top:3px;font-size:11px;color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.client-card small code{font-size:10.5px;color:var(--ink-soft)}
.client-mark{display:grid;place-items:center;width:34px;height:34px;border-radius:10px;background:#F0E9FF;color:#7552B8;flex-shrink:0}
.client-mark.claude_code{background:#FFF0E5;color:#B75B27}
.client-state{border-radius:20px;padding:4px 9px;font-size:10.5px;background:var(--fill);color:var(--ink-soft);flex-shrink:0}
.client-state.installed{background:#eaf6f0;color:#187e71}
.client-state.missing{background:#fbf5e8;color:#9c844c}
.client-actions{margin:8px 0 0 45px}
.client-link{border:0;background:none;padding:2px 0;font:inherit;font-size:11.5px;color:var(--teal);cursor:pointer}
.client-link:hover:not(:disabled){text-decoration:underline}
.client-link:disabled{opacity:.5;cursor:not-allowed}
.clients-unavailable{text-align:center;padding:34px 20px;color:var(--muted)}
.clients-unavailable>svg{margin:auto}
.clients-unavailable h3{font-size:17px;font-weight:550;margin:15px 0 9px;color:var(--ink)}
.clients-unavailable p{font-size:12px;line-height:1.9;color:var(--ink-soft);max-width:430px;margin:auto}
@media(pointer:coarse){.settings-lead-action{width:44px;height:44px}.client-link{min-height:44px}}
</style>
