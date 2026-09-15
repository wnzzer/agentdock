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
    <div class="clients-intro"><span><Icon name="spark" :size="21"/></span><div><strong>{{ t('Agent clients') }}</strong><p>{{ t('AgentDock runs the official Claude Code and Codex clients. This page shows which ones this host has, and can install a missing one from npm into AgentDock\'s own directory.') }}</p></div></div>

    <div v-if="!supported||unavailable" class="clients-unavailable"><Icon name="settings" :size="25"/><h3>{{ t('Client management needs an updated backend') }}</h3><p>{{ t('Sessions still launch whatever client is already on this host. Nothing is installed or changed here.') }}</p></div>
    <template v-else>
      <p v-if="error" class="account-error" role="alert">{{ error }}</p>
      <p v-if="notice" class="account-notice" role="status">{{ notice }}</p>
      <p v-if="loading&&!clients.length" class="clients-quiet">{{ t('Checking this host…') }}</p>

      <article v-for="client in clients" :key="client.provider" class="client-card">
        <header>
          <span :class="['client-mark',client.provider]"><ProviderIcon :provider="client.provider" :size="21"/></span>
          <div>
            <strong>{{ providerLabel(client.provider) }}</strong>
            <small>{{ client.version || t('No version reported') }} · {{ sourceLabel(client) }}</small>
          </div>
          <span :class="['client-state',client.installed?'installed':'missing']">{{ t(client.installed?'Installed':'Not installed') }}</span>
        </header>
        <dl class="client-facts">
          <div><dt>{{ t('Program') }}</dt><dd><code>{{ client.program }}</code></dd></div>
          <div><dt>{{ t('npm package') }}</dt><dd><code>{{ client.npm_package }}</code></dd></div>
          <div v-if="client.managed_path"><dt>{{ t('AgentDock copy') }}</dt><dd><code>{{ client.managed_path }}</code></dd></div>
        </dl>
        <p v-if="client.shadowed" class="clients-quiet">{{ t('AgentDock has its own copy, but the host PATH copy wins. Installing again will not change which client sessions run.') }}</p>
        <div v-if="confirming===client.provider" class="account-confirm">
          <strong>{{ t('Install {name} from npm?', { name: providerLabel(client.provider) }) }}</strong>
          <p>{{ t('This downloads {package} from the npm registry and runs its install scripts on this host. It installs into AgentDock\'s own directory, not globally, and never replaces a client already on PATH.', { package: client.npm_package }) }}</p>
          <div class="account-buttons"><button :disabled="!!busy" @click="confirming=undefined">{{ t('Cancel') }}</button><button class="account-primary" :disabled="!!busy" @click="install(client)">{{ t(busy===client.provider?'Installing…':'Install now') }}</button></div>
        </div>
        <div v-else class="client-actions">
          <button class="secondary-button" :disabled="!!busy||loading" @click="load">{{ t('Re-check') }}</button>
          <button v-if="client.install_available" class="secondary-button" :disabled="!!busy" @click="confirming=client.provider;notice=''">{{ t(client.installed?'Install or update AgentDock copy…':'Install from npm…') }}</button>
          <small v-else class="clients-quiet">{{ t('npm was not found on this host, so installing here is unavailable.') }}</small>
        </div>
      </article>
    </template>
  </section>
</template>

<style scoped>
.clients-page{min-width:0}
.clients-intro{display:flex;align-items:flex-start;gap:13px;margin-bottom:20px;color:#273745}
.clients-intro>span{display:grid;place-items:center;width:43px;height:43px;border-radius:13px;background:var(--teal-soft);flex-shrink:0}
.clients-intro strong{font-size:15px;font-weight:550}
.clients-intro p{font-size:12px;color:var(--muted);line-height:1.8;margin:6px 0 0}
.clients-quiet{font-size:10px;line-height:1.8;color:var(--muted);margin:8px 0 0}
.client-card{border:1px solid var(--border);border-radius:12px;padding:15px;margin-bottom:13px;background:#fbfcfd}
.client-card header{display:flex;align-items:center;gap:11px;flex-wrap:wrap}
.client-card header>div{flex:1;min-width:0}
.client-card strong{display:block;font-size:13px;font-weight:550;color:#273745}
.client-card small{display:block;margin-top:4px;font-size:10px;color:var(--muted);overflow-wrap:anywhere}
.client-mark{display:grid;place-items:center;width:35px;height:35px;border-radius:11px;background:#F0E9FF;color:#7552B8;flex-shrink:0}
.client-mark.claude_code{background:#FFF0E5;color:#B75B27}
.client-state{border-radius:20px;padding:5px 10px;font-size:10px;background:#f1f4f6;color:#647681}
.client-state.installed{background:#eaf6f0;color:#187e71}
.client-state.missing{background:#fbf5e8;color:#9c844c}
.client-facts{margin:13px 0 0;display:grid;gap:7px}
.client-facts>div{display:flex;gap:10px;font-size:10px;min-width:0}
.client-facts dt{color:var(--muted);width:104px;flex-shrink:0}
.client-facts dd{margin:0;min-width:0}
.client-facts code{font-size:10px;color:#647681;overflow-wrap:anywhere}
.client-actions{display:flex;align-items:center;flex-wrap:wrap;gap:9px;margin-top:14px}
.client-actions button{min-height:44px;padding:8px 12px;font-size:11px}
.clients-unavailable{text-align:center;padding:34px 20px;color:var(--muted)}
.clients-unavailable>svg{margin:auto}
.clients-unavailable h3{font-size:17px;font-weight:550;margin:15px 0 9px;color:#273745}
.clients-unavailable p{font-size:12px;line-height:1.9;color:#647681;max-width:430px;margin:auto}
@media(max-width:680px){.client-facts>div{flex-direction:column;gap:3px}.client-facts dt{width:auto}.clients-intro strong{font-size:14px}}
</style>
