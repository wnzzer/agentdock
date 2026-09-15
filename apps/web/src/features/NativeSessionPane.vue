<script setup lang="ts">
import { computed, defineAsyncComponent, ref, watch } from "vue";
import type { EndpointProfile, ProviderKind, Session } from "@agentdock/protocol";
import { errorMessage, json, providerLabel, request } from "./api";
import { sessionConnections } from "./session-connection";
import { backendCapabilities } from "./backend-capabilities";
import { useI18n } from "../i18n";
import Icon from "./Icon.vue";
import ProviderIcon from "./ProviderIcon.vue";
import { useSessionMenuPosition } from "./session-menu-position";
const TerminalPane=defineAsyncComponent(()=>import("../components/TerminalPane.vue"));
const { t }=useI18n();
const props=withDefaults(defineProps<{paneId?:string;session?:Session;sessions:Session[];profiles:EndpointProfile[];terminal?:boolean;consumeOpenIntent?:boolean}>(),{consumeOpenIntent:true});
const emit=defineEmits<{changed:[];create:[provider?:ProviderKind];select:[session:Session];environment:[id:string];renameRequest:[id:string];structured:[id:string]}>();
const terminalView=ref<{reconnect:()=>void}>();
const ending=ref(false),confirmEnd=ref(false),error=ref("");
const menu=ref<HTMLDetailsElement>();
const { tabStyle: tabMenuStyle, panelStyle: tabMenuPanelStyle, positionMenu } = useSessionMenuPosition(() => props.paneId, menu);
const entry=computed(()=>props.session?sessionConnections.get(props.session.id):undefined);
const profile=computed(()=>props.session?.endpoint_snapshot??props.profiles.find(p=>p.id===props.session?.endpoint_profile_id));
const nativeConfig=computed(()=>profile.value?.native_config);
const unboundImportedConfig=computed(()=>props.session && !props.session.endpoint_profile_id && !props.session.native_source_id && !nativeConfig.value && props.profiles.some(profile=>profile.provider===props.session!.provider && !!profile.native_config));
const choices=computed(()=>props.sessions.filter(s=>props.terminal?s.provider==="terminal":s.provider!=="terminal"));
const running=computed(()=>entry.value?.state==="attached");
const streamConnected=ref(false);
watch([()=>props.session?.id,running],()=>{streamConnected.value=false;},{flush:"sync"});
const terminalStatus=computed(()=>{const id=props.session?.id;return(connected:boolean)=>{if(props.session?.id===id)streamConnected.value=connected;};});
watch([()=>props.session?.id,()=>entry.value?.epoch,()=>props.session?.status,()=>props.session?.updated_at],async(_current,_previous,onCleanup)=>{
  let current=true;onCleanup(()=>{current=false;});
  const s=props.session;error.value="";confirmEnd.value=false;
  if(_current[0]!==_previous?.[0])ending.value=false;
  if(!s)return;
  await (props.consumeOpenIntent ? sessionConnections.ensure(s) : sessionConnections.attach(s));
  if(current&&props.session?.id===s.id)emit("changed");
},{immediate:true});
function retry(){const s=props.session;if(!s||ending.value||entry.value?.pending)return;sessionConnections.requestOpen(s.id);void sessionConnections.ensure(s);}
const terminalExited=computed(()=>{const id=props.session?.id;return()=>{if(id)sessionConnections.ended(id);emit("changed");};});
async function endSession(){
  const s=props.session;if(!s||ending.value)return;ending.value=true;error.value="";
  try{await request(`/sessions/${encodeURIComponent(s.id)}/stop`,json("POST"));sessionConnections.ended(s.id);if(props.session?.id===s.id)confirmEnd.value=false;emit("changed");}
  catch(cause){if(props.session?.id===s.id)error.value=errorMessage(cause);}finally{if(props.session?.id===s.id)ending.value=false;}
}
function closeSessionMenu(restoreFocus=false){if(menu.value){menu.value.open=false;if(restoreFocus)menu.value.querySelector('summary')?.focus();}}
function reconnect(){closeSessionMenu();terminalView.value?.reconnect();}
</script>
<template>
  <section v-if="!session" class="pane-empty session-picker">
    <span class="empty-icon"><Icon :name="terminal?'terminal':'spark'" :size="28" /></span>
    <h3>{{ t(terminal?'A terminal, when you need it':'Make room for your next idea') }}</h3>
    <p>{{ t('Open a session to connect. Closing the pane keeps it running.') }}</p>
    <button class="primary-button" @click="emit('create',terminal?'terminal':'claude_code')"><Icon name="plus" :size="15" />{{ t(terminal?'New terminal':'New agent session') }}</button>
    <div v-if="choices.length" class="session-choices"><small>{{ t('OR OPEN AN EXISTING SESSION') }}</small><button v-for="s in choices" :key="s.id" @click="emit('select',s)"><ProviderIcon :provider="s.provider" :size="15" /><span>{{ s.title }}</span><span :class="['state-dot',s.status]" /></button></div>
  </section>
  <section v-else class="agent-session-pane">
    <div v-if="!paneId" class="session-control">
      <div class="session-identity"><span :class="['provider-mark',session.provider]"><ProviderIcon :provider="session.provider" /></span><div><strong>{{ session.provider==='terminal'?t('Terminal'):providerLabel(session.provider) }}</strong><small :title="nativeConfig?.config_dir">{{ session.native_source_id?t('Original client configuration'):nativeConfig?t('{name} · shared host configuration',{name:profile?.name??providerLabel(session.provider)}):profile?.name||t(session.provider==='terminal'?'Host shell':'Isolated native profile') }}</small></div></div>
      <div class="session-actions">
        <span class="status-pill" :class="running?'running':session.status">{{ t(entry?.state==='opening'?'Connecting…':running?(streamConnected?'Connected':'Process running'):entry?.state==='failed'?'Connection failed':'Session ended') }}</span>
        <details ref="menu" class="session-menu" @keydown.esc.stop.prevent="closeSessionMenu(true)">
          <summary class="icon-button" :aria-label="t('Session actions')">⋯</summary>
          <div class="session-menu-content"><button v-if="running" :disabled="ending" @click="reconnect">{{ t('Reconnect view') }}</button><button v-else :disabled="!!entry?.pending||ending" @click="retry(); closeSessionMenu()">{{ t('Reopen session') }}</button><button :disabled="ending" @click="emit('environment',session.id); closeSessionMenu()">{{ t('Session environment') }}</button><slot name="session-actions" :close-menu="closeSessionMenu" /><button v-if="running" class="danger-text" :disabled="ending" @click="confirmEnd=true; closeSessionMenu()">{{ t('End session…') }}</button></div>
        </details>
      </div>
    </div>
    <Teleport v-if="paneId" to="body"><details ref="menu" class="session-menu session-tab-menu" :style="tabMenuStyle" @toggle="positionMenu" @keydown.esc.stop.prevent="closeSessionMenu(true)">
      <summary :aria-label="t('Session actions')" :title="t('Session actions')"><Icon name="more" :size="17" /></summary>
      <div class="session-menu-content" :style="tabMenuPanelStyle"><div class="session-menu-status"><ProviderIcon :provider="session.provider" :size="14" />{{ t(entry?.state==='opening'?'Connecting…':running?(streamConnected?'Connected':'Process running'):entry?.state==='failed'?'Connection failed':'Session ended') }}</div><button v-if="running" :disabled="ending" @click="reconnect"><Icon name="refresh" :size="14" />{{ t('Reconnect view') }}</button><button v-else :disabled="!!entry?.pending||ending" @click="retry(); closeSessionMenu()"><Icon name="play" :size="14" />{{ t('Reopen session') }}</button><button :disabled="ending" @click="emit('environment',session.id); closeSessionMenu()"><Icon name="settings" :size="14" />{{ t('Session environment') }}</button><slot name="session-actions" :close-menu="closeSessionMenu" /><button v-if="running" class="danger-text" :disabled="ending" @click="confirmEnd=true; closeSessionMenu()"><Icon name="stop" :size="14" />{{ t('End session…') }}</button></div>
    </details></Teleport>
    <div v-if="error||entry?.error" class="inline-error" role="alert">{{ error||entry?.error }}</div>
    <div v-if="backendCapabilities.structuredChat && session.provider!=='terminal'" class="inline-notice chat-upgrade-notice"><span>{{ t(running?'This existing terminal keeps running. End it before switching to the mobile chat UI.':'Switch this stopped session to the mobile chat UI. The native CLI still owns tools and approvals.') }}</span><button class="small-button" :disabled="running||ending||!!entry?.pending" @click="emit('structured',session.id)">{{ t('Use chat UI') }}</button></div>
    <div v-if="unboundImportedConfig" class="inline-notice">{{ t('This session uses isolated configuration, not the imported host configuration. Importing does not change existing sessions; select the host profile when creating a new session.') }}</div>
    <div v-if="session.native_source_id" class="inline-notice">{{ t('Loaded history uses the original client settings. It does not take over another running terminal.') }}</div>
    <details v-else-if="nativeConfig" class="inline-notice shared-session-details"><summary>{{ t('New session · shared host configuration') }}</summary><code>{{ nativeConfig.config_dir }}</code><p>{{ t('This is a new session using the host configuration, not a resumed history session.') }}</p><p>{{ t('Account, model, endpoint, proxy and permissions follow the native client. Shared configuration is not session-isolated.') }}</p><p>{{ t('This is a live directory reference, not a snapshot of its contents. Changes to native settings affect subsequent starts.') }}</p><p>{{ t('AgentDock does not copy credentials or rewrite this configuration. The native client may update its own sign-in cache and history.') }}</p></details>
    <div v-if="confirmEnd" class="confirmation-bar">{{ t('End this session? Its background process will stop; history remains.') }}<button class="small-button danger" :disabled="ending" @click="endSession">{{ t('End session') }}</button><button class="small-button" :disabled="ending" @click="confirmEnd=false">{{ t('Keep connected') }}</button></div>
    <TerminalPane v-if="running" :key="session.id" ref="terminalView" :session-id="session.id" :dark="session.provider==='terminal'" @exit="terminalExited" @status="terminalStatus" />
    <div v-else class="pane-empty"><ProviderIcon :provider="session.provider" :size="28" /><h3>{{ t(entry?.state==='opening'?'Connecting…':entry?.state==='failed'?'Connection failed':'Session ended') }}</h3><p>{{ t(entry?.state==='opening'?'Connecting to the native client.':'Reopen this session to continue. No process is restarted automatically after it ends.') }}</p><button v-if="entry?.state!=='opening'" class="secondary-button" :disabled="!!entry?.pending||ending" @click="retry">{{ t('Reopen session') }}</button></div>
  </section>
</template>
<style scoped>
.session-menu.session-tab-menu>summary{min-height:28px}.session-menu-content>button>svg{margin-right:7px}.session-menu-status{display:flex;align-items:center;gap:6px;font-size:11px;color:var(--muted);padding:8px 9px;border-bottom:1px solid var(--border)}.session-menu summary:focus-visible,.session-menu button:focus-visible{outline:2px solid #51b4a3;outline-offset:2px}
@media(max-width:520px){.session-menu.session-tab-menu>summary{min-width:32px;min-height:32px;width:32px;height:32px}}
</style>
<style scoped>.session-menu{position:relative}.session-menu>summary{list-style:none;cursor:pointer;min-width:44px;min-height:44px}.session-menu>summary::-webkit-details-marker{display:none}.session-menu-content{position:absolute;right:0;top:44px;z-index:25;width:190px;background:#fff;border:1px solid #dce5e8;border-radius:8px;padding:5px;box-shadow:0 10px 24px #18373a20}.session-menu-content>button{display:block;width:100%;min-height:44px;border:0;background:none;padding:8px;text-align:left;font-size:12px;color:#42606b;cursor:pointer}.session-menu-content>button:hover{background:#edf6f3}.session-tab-menu{position:fixed;z-index:60;display:inline-flex}.session-tab-menu>summary{min-width:28px;width:28px;height:28px;border-radius:6px;display:grid;place-items:center;font-size:18px}.session-tab-menu>summary:hover,.session-tab-menu[open]>summary{background:#eaf6f3;color:#087e73}.session-tab-menu .session-menu-content{top:32px}.shared-session-details{flex-shrink:0;max-height:40%;overflow:auto}.shared-session-details>summary{cursor:pointer;color:#4c8072}.shared-session-details>code{display:block;overflow-wrap:anywhere;margin-top:8px;font-size:10px}.shared-session-details p{margin-top:7px;line-height:17px}</style>
