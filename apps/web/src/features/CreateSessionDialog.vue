<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { EndpointProfile, ModelCatalog, ProviderKind, Session, Workspace } from "@agentdock/protocol";
import { errorMessage, json, providerLabel, request, workspacePath } from "./api";
import { sessionEffortOverride, sessionModelOverride } from "./native-profiles";
import { effortLabel, modelEfforts } from "./reasoning-effort";
import { PROFILE_CHOICE_REQUIRED, isProfileSelectionValid, preferredProfileSelection, rememberProfileSelection } from "./profile-preferences";
import { useI18n } from "../i18n";
import { backendCapabilities } from "./backend-capabilities";
import { environmentRows, mergeEnvironment, parseEnvironmentRows, type EnvironmentRow } from "./environment-model";
import EnvironmentEditor from "./EnvironmentEditor.vue";
import ModalDialog from "./ModalDialog.vue";
import ProviderIcon from "./ProviderIcon.vue";
import ModelPicker from "./ModelPicker.vue";
const { t }=useI18n();
const props = defineProps<{ workspace: Workspace; profiles: EndpointProfile[]; initialProvider?: ProviderKind }>();
const emit = defineEmits<{ close: []; created: [session: Session]; profiles: [] }>();
const provider = ref<ProviderKind>(props.initialProvider ?? "claude_code");
const title = ref(t('{provider} session',{provider:providerLabel(provider.value)}));
const profileId=ref(preferredProfileSelection(provider.value,props.profiles)), profileTouched=ref(false), model=ref(""), effort=ref(""), busy=ref(false),error=ref(""),discovering=ref(false),modelError=ref("");
const catalog=ref<ModelCatalog>();
const available = computed(() => props.profiles.filter(item => item.provider === provider.value));
const selectedProfile = computed(() => available.value.find(item => item.id === profileId.value));
const nativeConfig = computed(() => selectedProfile.value?.native_config);
const validProfile = computed(() => isProfileSelectionValid(provider.value,profileId.value,props.profiles));
const choices=computed(()=>nativeConfig.value?[]:[...Object.entries(selectedProfile.value?.model_aliases??{}).map(([id,name])=>({id,name:id+" → "+name})),...(catalog.value?.models??[])]);
const effectiveModel=computed(()=>{if(nativeConfig.value)return undefined;const selected=model.value||selectedProfile.value?.model;return selected?(selectedProfile.value?.model_aliases?.[selected]??selected):undefined});
const effortOptions=computed(()=>provider.value==='terminal'?[]:modelEfforts(provider.value,effectiveModel.value,catalog.value,effort.value||selectedProfile.value?.effort));
const ephemeral=ref(false);
const ephemeralSupported=computed(()=>backendCapabilities.ephemeralSessions);
const environmentDrafts=ref(new Map<string,EnvironmentRow[]>());
const environmentScope=computed(()=>JSON.stringify([props.workspace.id,provider.value,profileId.value]));
const environmentDraft=computed({get:()=>environmentDrafts.value.get(environmentScope.value)??[],set:(rows:EnvironmentRow[])=>{environmentDrafts.value.set(environmentScope.value,rows);}});
const parsedEnvironment=computed(()=>parseEnvironmentRows(environmentDraft.value));
const mergedEnvironment=computed(()=>parseEnvironmentRows(environmentRows(mergeEnvironment(selectedProfile.value?.environment,parsedEnvironment.value.environment))));
const environmentErrors=computed(()=>[...new Set([...parsedEnvironment.value.errors,...mergedEnvironment.value.errors])]);
const unsupportedEnvironment=computed(()=>!backendCapabilities.environment&&(environmentDraft.value.length>0||Object.keys(selectedProfile.value?.environment??{}).length>0));
const validEnvironment=computed(()=>!unsupportedEnvironment.value&&!environmentErrors.value.length);
let revision=0;
watch(provider,(value,previous)=>{if(title.value===t('{provider} session',{provider:providerLabel(previous)})) title.value=t('{provider} session',{provider:providerLabel(value)});profileTouched.value=false;profileId.value=preferredProfileSelection(value,props.profiles);model.value="";effort.value="";},{flush:"sync"});
watch(available,()=>{
  if(busy.value)return;
  if(profileId.value && profileId.value!==PROFILE_CHOICE_REQUIRED){
    if(!validProfile.value){profileTouched.value=true;profileId.value=PROFILE_CHOICE_REQUIRED;}
    return; // Refreshes must not replace a valid account already displayed in the form.
  }
  if(!profileTouched.value)profileId.value=preferredProfileSelection(provider.value,props.profiles);
},{flush:"sync"});
watch([provider,profileId,()=>nativeConfig.value?.source_id,()=>nativeConfig.value?.config_dir],()=>{catalog.value=undefined;modelError.value="";model.value="";effort.value=selectedProfile.value?.effort??"";discovering.value=false;revision++;},{flush:"sync"});
async function discover(){
  if(!validProfile.value||!backendCapabilities.models||nativeConfig.value||provider.value==="terminal")return;
  const p=selectedProfile.value ?? {name:"Native catalog",provider:provider.value,endpoint_url:null,model:null,permission_mode:"native",secret_ref:null,proxy_url:null,effort:null,model_aliases:{}};if(discovering.value)return;
  discovering.value=true;modelError.value="";const own=++revision;
  try{const result=await request<ModelCatalog>("/endpoint-profiles/discover-models",json("POST",{name:p.name,provider:p.provider,endpoint_url:p.endpoint_url,model:p.model,permission_mode:p.permission_mode,secret_ref:p.secret_ref,proxy_url:p.proxy_url??null,effort:p.effort??null,model_aliases:p.model_aliases??{}}));if(own===revision)catalog.value=result;}
  catch(cause){if(own===revision)modelError.value=errorMessage(cause);}
  finally{if(own===revision)discovering.value=false;}
}
async function create() {
  if (!title.value.trim() || busy.value || !validProfile.value || !validEnvironment.value) return;
  const selectedProvider=provider.value, selectedProfileId=profileId.value;
  busy.value = true; error.value = "";
  try { const session = await request<Session>(`${workspacePath(props.workspace.id)}/sessions`, json("POST", { title: title.value.trim(), provider: selectedProvider, endpoint_profile_id: selectedProfileId || null, ...(backendCapabilities.structuredChat&&selectedProvider!=='terminal'?{interaction_mode:'structured'}:{}), ...sessionModelOverride(selectedProvider,selectedProfile.value,model.value), ...sessionEffortOverride(selectedProvider,selectedProfile.value,effort.value), ...(backendCapabilities.environment&&Object.keys(parsedEnvironment.value.environment).length?{environment:parsedEnvironment.value.environment}:{}), ...(ephemeralSupported.value&&ephemeral.value?{ephemeral:true}:{}) })); rememberProfileSelection(selectedProvider,selectedProfileId); emit("created", session); }
  catch (cause) { error.value = errorMessage(cause); }
  finally { busy.value = false; }
}
onBeforeUnmount(()=>{revision++;});
</script>
<template>
  <ModalDialog :title="t('New session')" :closable="!busy" @close="!busy && emit('close')">
    <form class="form-stack" @submit.prevent="create">
      <p class="form-description">{{ t('Create a session in {workspace}. It connects when opened.',{workspace:workspace.name}) }}</p>
      <p v-if="backendCapabilities.structuredChat && provider!=='terminal'" class="inline-notice">{{ t('Chat UI · mobile ready. The official CLI runs the agent and handles permissions; no prompt is sent until you press Send.') }}</p>
      <label>{{ t('Provider') }}<select v-model="provider" :disabled="busy"><option value="claude_code">Claude Code</option><option value="codex">Codex</option><option value="terminal">{{ t('Terminal · optional host shell') }}</option></select></label>
      <label>{{ t('Session name') }}<input v-model="title" autofocus required :disabled="busy" maxlength="120" :placeholder="t('What are you working on?')" /></label>
      <template v-if="provider!=='terminal'">
        <label>{{ t('Endpoint profile') }}<select v-model="profileId" :disabled="busy" @change="profileTouched=true"><option v-if="profileId===PROFILE_CHOICE_REQUIRED" :value="PROFILE_CHOICE_REQUIRED" disabled>{{ t('Choose a session configuration') }}</option><option value="">{{ t('Isolated configuration · separate sign-in may be required') }}</option><option v-for="profile in available" :key="profile.id" :value="profile.id">{{ profile.name }} · {{ profile.native_config ? t('Host configuration · shared sign-in') : profile.model || t('default model') }}</option></select></label>
        <p v-if="!validProfile" class="inline-notice">{{ t('Choose a configuration explicitly when several host accounts are available. No account will be selected silently.') }}</p>
        <p v-else-if="nativeConfig" class="form-help">{{ t('This session will use {name} and its existing sign-in, if still valid.',{name:selectedProfile?.name??''}) }}</p>
        <div v-if="nativeConfig" class="inline-notice native-session-notice"><strong><ProviderIcon :provider="provider" :size="15" />{{ t('New session · shared host configuration') }}</strong><code>{{ nativeConfig.config_dir }}</code><p>{{ t('Account, model, endpoint, proxy and permissions follow the native client. Shared configuration is not session-isolated.') }}</p><p>{{ t('This is a new session using the host configuration, not a resumed history session.') }}</p><p>{{ t('Directory availability does not confirm sign-in. The native client may still ask you to log in.') }}</p><p>{{ t('This is a live directory reference, not a snapshot of its contents. Changes to native settings affect subsequent starts.') }}</p><p>{{ t('AgentDock does not copy credentials or rewrite this configuration. The native client may update its own sign-in cache and history.') }}</p></div>
        <template v-else-if="validProfile">
        <div v-if="selectedProfile" class="inline-notice"><ProviderIcon :provider="provider" :size="15" /> {{ selectedProfile.endpoint_url || t('Official endpoint') }}<br />{{ t('Permission intent') }}: {{ t(selectedProfile.permission_mode) }} · {{ t('This session keeps a configuration snapshot.') }}<br v-if="selectedProfile.proxy_url" /><span v-if="selectedProfile.proxy_url">{{ t('Proxy URL') }}: {{ selectedProfile.proxy_url }}</span></div>
        <div class="session-model-bar"><label>{{ t('Session model') }}<ModelPicker v-model="model" :models="choices" :placeholder="selectedProfile?.model || t('Use profile or native default')" /></label><button v-if="selectedProfile || provider==='codex'" type="button" class="small-button" :disabled="discovering||!backendCapabilities.models" :title="!backendCapabilities.models?t('Backend upgrade required'):undefined" @click="discover">{{ discovering?t('Loading models…'):t('Load models') }}</button></div>
        <p v-if="catalog?.source_url==='codex://model/list'" class="form-help">{{ t('Loaded from the native Codex catalog. Account access may differ.') }}</p>
        <div v-if="modelError" class="inline-error" role="alert">{{ modelError }}</div>
        <p v-if="effectiveModel" class="form-help">{{ t('Effective model') }}: <code>{{ effectiveModel }}</code></p>
        <label v-if="!nativeConfig && effortOptions.length">{{ t('Thinking depth') }}<select v-model="effort"><option value="">{{ t('Automatic · provider default') }}</option><option v-for="level in effortOptions" :key="level" :value="level">{{ effortLabel(level) }}</option></select></label>
        <p v-if="!nativeConfig && !effortOptions.length" class="form-help">{{ t('Thinking depth is unavailable until this model advertises supported reasoning levels.') }}</p>
        <p class="form-help">{{ t('This override affects only the new session, not the profile or other sessions.') }}</p>
        <p v-if="!selectedProfile" class="form-help">{{ t('The client uses a session-specific configuration directory. Sign in through the native client; existing global credentials are not copied.') }}</p>
        </template>
        <button type="button" class="text-button" :disabled="busy" @click="emit('profiles')">{{ t('Manage endpoint profiles') }}</button>
      </template>
      <label v-if="ephemeralSupported" class="session-ephemeral-choice"><input v-model="ephemeral" type="checkbox" :disabled="busy" /><span><strong>{{ t('Temporary window') }}</strong><small>{{ t('Discarded when its window is closed, and cleared on restart. It still appears in its workspace, marked temporary, so you can keep it at any time.') }}</small></span></label>
      <EnvironmentEditor v-model="environmentDraft" :inherited="selectedProfile?.environment" :supported="backendCapabilities.environment" :disabled="busy||!validProfile" />
      <p class="form-help">{{ t('Environment drafts stay with this provider and profile. Switching accounts does not carry overrides across.') }}</p>
      <p class="form-help">{{ t('Environment overrides affect only the launched process, not the native configuration files.') }}</p>
      <div v-for="message in environmentErrors" :key="message" class="inline-error" role="alert">{{ t(message) }}</div>
      <div v-if="unsupportedEnvironment" class="inline-notice" role="alert">{{ t('Environment overrides require an updated backend. Your draft has not been discarded.') }}<button v-if="environmentDraft.length" type="button" class="text-button" :disabled="busy" @click="environmentDraft=[]">{{ t('Discard environment draft') }}</button></div>
      <div v-if="error" class="inline-error" role="alert">{{ error }}</div>
      <div class="dialog-actions"><button type="button" class="secondary-button" :disabled="busy" @click="emit('close')">{{ t('Cancel') }}</button><button class="primary-button" :disabled="busy || !title.trim() || !validProfile || !validEnvironment">{{ busy?t('Creating…'):t('Create session') }}</button></div>
    </form>
  </ModalDialog>
</template>
<style scoped>.session-ephemeral-choice{flex-direction:row;align-items:flex-start;gap:9px;padding:9px 11px;border:1px solid #e6e1f1;border-radius:7px;background:#faf8fd}.session-ephemeral-choice input[type=checkbox]{width:14px;height:14px;min-width:14px;margin:2px 0 0;flex:0 0 14px;accent-color:#8973b4}.session-ephemeral-choice strong{display:block;font-size:10px;font-weight:550;color:#6f5c93}.session-ephemeral-choice small{display:block;margin-top:3px;font-size:9px;line-height:15px;color:#9a93ad;font-weight:400}
.session-model-bar{display:flex;align-items:end;gap:8px}.session-model-bar>label{flex:1;min-width:0}.session-model-bar>button{margin-bottom:1px;white-space:nowrap}.native-session-notice>strong{display:flex;align-items:center;gap:6px;font-weight:550;color:#397d6c}.native-session-notice>code{display:block;margin:8px 0;font-size:10px;overflow-wrap:anywhere}.native-session-notice p{margin:7px 0 0;line-height:18px}</style>
