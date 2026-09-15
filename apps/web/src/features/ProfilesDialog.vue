<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from "vue";
import type { AgentProviderKind, EndpointProfile, ModelCatalog, NativeHistorySource } from "@agentdock/protocol";
import { errorMessage, json, providerLabel, request } from "./api";
import { parseModelAliases, formatModelAliases } from "./endpoint-models";
import { isSameNativeSource, nativeProfileImportPayload, nativeProfileRenamePayload, nativeProfileUpdatePayload, profileEnvironmentPayload, profileEnvironmentDraftChanged } from "./native-profiles";
import { environmentRows, type EnvironmentRow } from "./environment-model";
import EnvironmentEditor from "./EnvironmentEditor.vue";
import { rememberProfileSelection } from "./profile-preferences";
import { effortLabel, modelEfforts } from "./reasoning-effort";
import { useI18n } from "../i18n";
import { backendCapabilities } from "./backend-capabilities";
import ModalDialog from "./ModalDialog.vue";
import Icon from "./Icon.vue";
import ProviderIcon from "./ProviderIcon.vue";
import ModelPicker from "./ModelPicker.vue";
const { t } = useI18n();
const props = defineProps<{ profiles: EndpointProfile[]; embedded?: boolean }>();
const embedded = computed(() => props.embedded === true);
const emit = defineEmits<{ close: []; changed: [profile?: EndpointProfile] }>();
// The settings shell owns the outer close, so it needs this page's unsaved guard.
defineExpose({ requestLeave: (action: () => void) => requestLeave(action) });
const editing = ref<string>(), deleteId = ref<string>();
const editingRecord = ref<EndpointProfile>();
const editingProfile = computed(() => props.profiles.find(profile => profile.id === editing.value) ?? editingRecord.value);
const editingNative = computed(() => editingProfile.value?.native_config);
const editingManaged = computed(() => editingNative.value?.source_id.startsWith('account:'));
const formVisible = ref(!props.profiles.length), busy = ref(false), discovering = ref(false);
const importVisible = ref(false), loadingSources = ref(false), sourcesLoaded = ref(false), sharedConfirmed = ref(false);
const nativeSources = ref<NativeHistorySource[]>([]), nativeSourceId = ref(""), nativeName = ref("");
const selectedSource = computed(() => nativeSources.value.find(source => source.id === nativeSourceId.value));
const existingNativeProfile = computed(() => props.profiles.find(profile => isSameNativeSource(profile, selectedSource.value)));
const error = ref(""), notice = ref(""), modelError = ref("");
const models = ref<ModelCatalog>(), aliasesText = ref("");
const environmentDraft = ref<EnvironmentRow[]>([]), initialEnvironment = ref<EnvironmentRow[]>([]);
const environmentDirty = computed(() => profileEnvironmentDraftChanged(environmentDraft.value, initialEnvironment.value));
const environmentError = computed(() => { try { profileEnvironmentPayload(environmentDraft.value, backendCapabilities.environment, initialEnvironment.value); return ""; } catch (cause) { return errorMessage(cause); } });
const pendingNavigation = ref<(() => void)>();
const form = reactive({ name: "", provider: "claude_code" as AgentProviderKind, endpoint_url: "", model: "", effort: "", permission_mode: "native" as EndpointProfile["permission_mode"], secret_ref: "", proxy_url: "" });
const secretValid = computed(() => !form.secret_ref || /^env:AGENTDOCK_SECRET_[A-Z0-9_]+$/.test(form.secret_ref));
const deletingProfile = computed(() => props.profiles.find(p => p.id === deleteId.value) ?? (editingRecord.value?.id === deleteId.value ? editingRecord.value : undefined));
const aliases = computed(() => { try { return { value: parseModelAliases(aliasesText.value), error: "" }; } catch (cause) { return { value: {} as Record<string,string>, error: errorMessage(cause) }; } });
const resolved = computed(() => aliases.value.value[form.model] ?? form.model);
const choices = computed(() => [...Object.entries(aliases.value.value).map(([id, actual]) => ({id,name:id + " → " + actual})), ...(models.value?.models ?? [])]);
const effortOptions = computed(() => modelEfforts(form.provider, resolved.value || undefined, models.value, form.effort));
const canSave = computed(() => !!form.name.trim() && !environmentError.value && (!!editingNative.value || (secretValid.value && !aliases.value.error)));
let discoveryRevision = 0, nativeRevision = 0, disposed = false;
function resetDiscovery() { discoveryRevision++; models.value = undefined; modelError.value = ""; discovering.value = false; }
watch(() => [form.provider, form.endpoint_url, form.proxy_url, form.secret_ref], resetDiscovery);
watch(nativeSourceId, () => { sharedConfirmed.value = false; nativeName.value = existingNativeProfile.value?.name ?? selectedSource.value?.label ?? ""; }, { flush: "sync" });
function payload(includeEnvironment = true) {
  if (editingNative.value) return includeEnvironment ? nativeProfileUpdatePayload(form.name, environmentDraft.value, backendCapabilities.environment, initialEnvironment.value) : nativeProfileRenamePayload(form.name);
  const environment = includeEnvironment ? profileEnvironmentPayload(environmentDraft.value, backendCapabilities.environment, initialEnvironment.value) : {};
  const old=editingProfile.value;
  return { name:form.name.trim(), provider:form.provider, endpoint_url:form.endpoint_url.trim()||null, model:form.model.trim()||null, permission_mode:form.permission_mode, secret_ref:form.secret_ref.trim()||null,
    ...(form.proxy_url.trim()||old?.proxy_url!==undefined ? {proxy_url:form.proxy_url.trim()||null}:{}),
    ...(Object.keys(aliases.value.value).length||old?.model_aliases!==undefined ? {model_aliases:aliases.value.value}:{}),
    ...(form.effort.trim()||old?.effort!==undefined ? {effort:form.effort.trim()||null}:{}), ...environment };
}
function requestLeave(action: () => void) {
  if (busy.value) return;
  if (formVisible.value && environmentDirty.value) { pendingNavigation.value = action; return; }
  action();
}
function discardEnvironmentDraft() { const action = pendingNavigation.value; pendingNavigation.value = undefined; environmentDraft.value = initialEnvironment.value.map(row => ({ ...row })); action?.(); }
function edit(p?: EndpointProfile) {
  pendingNavigation.value = undefined;
  environmentDraft.value = environmentRows(p?.environment);
  initialEnvironment.value = environmentDraft.value.map(row => ({ ...row }));
  nativeRevision++;loadingSources.value=false;importVisible.value=false;sharedConfirmed.value=false;resetDiscovery();editingRecord.value=p;
  editing.value=p?.id; formVisible.value=true; error.value=""; notice.value=""; deleteId.value=undefined;
  Object.assign(form,{name:p?.name??"",provider:p?.provider??"claude_code",endpoint_url:p?.endpoint_url??"",model:p?.model??"",effort:p?.effort??"",permission_mode:p?.permission_mode??"native",secret_ref:p?.secret_ref??"",proxy_url:p?.proxy_url??""});
  aliasesText.value=formatModelAliases(p?.model_aliases); models.value=undefined; modelError.value="";
}
async function discover() {
  if (!backendCapabilities.models || editingNative.value || importVisible.value || discovering.value || !secretValid.value || aliases.value.error) return;
  const revision=++discoveryRevision; discovering.value=true; modelError.value="";
  // Model discovery is not a child-process launch: never send draft environment
  // values or secret references to that endpoint.
  try { const result=await request<ModelCatalog>("/endpoint-profiles/discover-models",json("POST",payload(false))); if(revision===discoveryRevision) models.value=result; }
  catch(cause){if(revision===discoveryRevision)modelError.value=errorMessage(cause);}
  finally {if(revision===discoveryRevision)discovering.value=false;}
}
async function save() {
  if(busy.value||!canSave.value)return;
  busy.value=true;error.value="";notice.value="";
  try{
    const saved=await request<EndpointProfile>("/endpoint-profiles"+(editing.value?"/"+encodeURIComponent(editing.value):""),json(editing.value?"PATCH":"POST",payload()));
    notice.value=editingNative.value?(backendCapabilities.environment?"Profile updated. Process environment overrides were saved without changing the shared native configuration.":"Profile name updated. The shared native configuration was not changed."):editing.value?"Profile updated. Existing session snapshots are unchanged.":"Profile created. Select it when creating a session.";
    pendingNavigation.value=undefined;formVisible.value=false;emit("changed",saved);
  }catch(cause){error.value=errorMessage(cause);}finally{busy.value=false;}
}
async function remove(){
  if(!deleteId.value||busy.value)return;const id=deleteId.value,native=!!deletingProfile.value?.native_config;busy.value=true;error.value="";
  try{await request("/endpoint-profiles/"+encodeURIComponent(id),json("DELETE"));deleteId.value=undefined;if(editing.value===id){formVisible.value=false;editing.value=undefined;editingRecord.value=undefined;}notice.value=native?"Profile reference removed. Native settings, credentials and history remain on the host.":"Profile deleted; no credentials were deleted from the host.";emit("changed");}
  catch(cause){error.value=errorMessage(cause);}finally{busy.value=false;}
}
async function loadNativeSources() {
  const own=++nativeRevision;loadingSources.value=true;sourcesLoaded.value=false;error.value="";sharedConfirmed.value=false;nativeSources.value=[];nativeSourceId.value="";
  try {
    const sources=await request<NativeHistorySource[]>("/host/native-configurations");
    if(disposed||own!==nativeRevision||!importVisible.value)return;
    nativeSources.value=sources;sourcesLoaded.value=true;
    nativeSourceId.value=sources.find(source=>source.available&&!props.profiles.some(profile=>isSameNativeSource(profile,source)))?.id??sources.find(source=>source.available)?.id??"";
  }catch(cause){if(!disposed&&own===nativeRevision)error.value=errorMessage(cause);}
  finally{if(!disposed&&own===nativeRevision)loadingSources.value=false;}
}
function openImport() {
  if(busy.value||!backendCapabilities.nativeConfig)return;resetDiscovery();importVisible.value=true;formVisible.value=false;editing.value=undefined;editingRecord.value=undefined;deleteId.value=undefined;notice.value="";
  void loadNativeSources();
}
function closeImport() {nativeRevision++;importVisible.value=false;loadingSources.value=false;sharedConfirmed.value=false;}
async function importNative() {
  const data=nativeProfileImportPayload(selectedSource.value,existingNativeProfile.value?"":nativeName.value,sharedConfirmed.value);
  if(!data||busy.value||loadingSources.value)return;
  busy.value=true;error.value="";notice.value="";
  try {
    const profile=await request<EndpointProfile>("/endpoint-profiles/import-native",json("POST",data));
    if(disposed)return;
    rememberProfileSelection(profile.provider,profile.id);
    edit(profile);notice.value="Host configuration imported and selected for future new sessions in this browser. Existing sessions are unchanged.";emit("changed",profile);
  }catch(cause){if(!disposed)error.value=errorMessage(cause);}
  finally{if(!disposed)busy.value=false;}
}
onBeforeUnmount(()=>{disposed=true;nativeRevision++;discoveryRevision++;});
</script>

<template>
  <ModalDialog :title="t('Endpoint profiles')" wide :closable="!busy" :embedded="embedded" @close="requestLeave(() => emit('close'))">
    <p class="form-description">{{ t('Choose an isolated endpoint profile or reuse an existing host configuration. Host configurations stay shared.') }}</p>
    <p v-if="!backendCapabilities.nativeConfig" class="inline-notice">{{ t('Native configuration import and history loading require an updated backend.') }}</p>
    <div class="profiles-layout">
      <section class="profiles-list">
        <button class="secondary-button" :disabled="busy" @click="requestLeave(() => edit())"><Icon name="plus" :size="14" />{{ t('New profile') }}</button>
        <button v-for="p in profiles" :key="p.id" :class="['profile-card',{selected:editing===p.id&&formVisible}]" :disabled="busy" @click="requestLeave(() => edit(p))">
          <span :class="['provider-mark',p.provider]"><ProviderIcon :provider="p.provider" /></span>
          <span><strong>{{ p.name }}</strong><small>{{ providerLabel(p.provider) }} · {{ p.native_config ? t('Native settings') : p.model || t('Native client default') }}</small><small v-if="p.native_config" class="shared-profile-label">{{ t('Host configuration · shared sign-in') }}</small><small v-else>{{ t(p.permission_mode) }} · {{ p.proxy_url ? t('Proxy configured') : t('Host network') }}</small><small v-if="p.native_config" class="profile-path" :title="p.native_config.config_dir">{{ p.native_config.config_dir }}</small></span>
        </button>
        <p v-if="!profiles.length" class="small-empty">{{ t('No profiles yet. Native, isolated sessions also work without one.') }}</p>
      </section>
      <div class="profile-details">
        <div v-if="notice" class="inline-success" role="status">{{ t(notice) }}</div>
        <div v-if="error" class="inline-error" role="alert">{{ error }}</div>
        <div v-if="pendingNavigation" class="confirmation-bar" role="alert">{{ t('Discard unsaved environment changes?') }}<div class="toolbar-buttons"><button type="button" class="small-button danger" @click="discardEnvironmentDraft">{{ t('Discard environment changes') }}</button><button type="button" class="small-button" @click="pendingNavigation=undefined">{{ t('Keep editing') }}</button></div></div>
        <form v-if="importVisible" class="form-stack" @submit.prevent="importNative">
          <h3>{{ t('Import existing configuration') }}</h3>
          <p class="form-description">{{ t('Add Claude Code or Codex configuration already on this host as a reusable profile.') }}</p>
          <p class="form-help">{{ t('After importing, new sessions for this client will select this configuration by default. Existing sessions are unchanged.') }}</p>
          <div class="native-source-controls"><label>{{ t('Host configuration source') }}<select v-model="nativeSourceId" :disabled="busy||loadingSources"><option value="" disabled>{{ t('Select a host configuration') }}</option><option v-for="source in nativeSources" :key="source.id" :value="source.id" :disabled="!source.available">{{ source.label }} · {{ t(source.available?'Directory available':'Not found on this host') }}</option></select></label><button type="button" class="icon-button" :aria-label="t('Refresh host configurations')" :disabled="busy||loadingSources" @click="loadNativeSources"><Icon name="refresh" :size="16" /></button></div>
          <p v-if="loadingSources" class="form-help" role="status">{{ t('Loading host configurations…') }}</p>
          <p v-else-if="sourcesLoaded&&!nativeSources.some(source=>source.available)" class="form-help">{{ t('No usable native configuration directories were found. Configure the native client on this host, then refresh.') }}</p>
          <template v-if="selectedSource">
            <div class="shared-config-panel"><strong><ProviderIcon :provider="selectedSource.provider" :size="17" />{{ t('Host configuration · shared sign-in') }}</strong><code>{{ selectedSource.path }}</code><p>{{ t('Directory availability does not confirm sign-in. The native client may still ask you to log in.') }}</p></div>
            <label>{{ t('Name') }}<input v-model="nativeName" :disabled="busy||!!existingNativeProfile" :placeholder="selectedSource.label" maxlength="120" /></label>
            <p v-if="existingNativeProfile" class="form-help">{{ t('This source is already linked. Importing reuses the existing profile; you can rename it afterwards.') }}</p>
            <p class="form-help">{{ t('Import only creates a profile reference. It does not start the client or sign you in.') }}</p>
            <div class="shared-config-consent"><p>{{ t('Account, model, endpoint, proxy and permissions follow the native client. Shared configuration is not session-isolated.') }}</p><p>{{ t('AgentDock does not copy credentials or rewrite this configuration. The native client may update its own sign-in cache and history.') }}</p><p>{{ t('This is a live directory reference, not a snapshot of its contents. Changes to native settings affect subsequent starts.') }}</p><label><input v-model="sharedConfirmed" type="checkbox" :disabled="busy" />{{ t('I agree to reuse this shared host configuration for new sessions.') }}</label></div>
          </template>
          <div class="dialog-actions"><button type="button" class="secondary-button" :disabled="busy" @click="closeImport">{{ t('Cancel') }}</button><button class="primary-button" :disabled="busy||loadingSources||!selectedSource?.available||!sharedConfirmed">{{ t(busy?'Importing…':'Import configuration') }}</button></div>
        </form>
        <form v-else-if="formVisible" class="form-stack" @submit.prevent="save">
          <h3>{{ editing ? t('Edit profile') : t('Create profile') }}</h3>
          <div class="form-columns">
            <label>{{ t('Name') }}<input v-model="form.name" required :disabled="busy" :placeholder="t('Personal Codex')" maxlength="120" /></label>
            <label>{{ t('Provider') }}<select v-model="form.provider" :disabled="!!editing||busy"><option value="claude_code">Claude Code</option><option value="codex">Codex</option></select></label>
          </div>
          <template v-if="editingNative">
            <div class="shared-config-panel"><strong><ProviderIcon :provider="editingProfile!.provider" :size="17" />{{ t('Host configuration · shared sign-in') }}</strong><code>{{ editingNative.config_dir }}</code><p>{{ t('Native account and directory settings remain managed by the client. Advanced environment overrides only affect new child processes.') }}</p><p>{{ t('Edit the profile name and process environment here. Native configuration files and account directories are not changed.') }}</p></div>
            <p class="form-help">{{ t('Directory availability does not confirm sign-in. The native client may still ask you to log in.') }}</p>
            <p class="form-help">{{ t('AgentDock does not copy credentials or rewrite this configuration. The native client may update its own sign-in cache and history.') }}</p>
            <p class="form-help">{{ t('This is a live directory reference, not a snapshot of its contents. Changes to native settings affect subsequent starts.') }}</p>
          </template>
          <template v-else>
          <label>{{ t('Endpoint URL') }} <small>{{ t('optional') }}</small><input v-model="form.endpoint_url" type="url" :placeholder="t('Official endpoint when empty')" autocomplete="off" /></label>
          <label>{{ t('Proxy URL') }} <small>HTTP / HTTPS</small><input v-model="form.proxy_url" type="url" placeholder="http://127.0.0.1:7890" autocomplete="off" /></label>
          <p class="form-help">{{ t('The proxy runs on the server, not in this browser. Empty uses the host network. Claude Code does not support SOCKS.') }}</p>
          <label>{{ t('Secret reference') }} <small>{{ t('never an API key') }}</small><input v-model="form.secret_ref" placeholder="env:AGENTDOCK_SECRET_PROVIDER_KEY" autocomplete="off" spellcheck="false" /></label>
          <p :class="secretValid?'form-help':'inline-error'">{{ secretValid?t('MVP supports env:AGENTDOCK_SECRET_NAME from the server environment. Secret values are not returned to the browser.'):t('Enter an environment reference, not the secret itself.') }}</p>
          <div class="model-fetch"><strong>{{ t('Model selection') }}</strong><button type="button" class="small-button" :disabled="discovering||!backendCapabilities.models||!secretValid||!!aliases.error" :title="!backendCapabilities.models?t('Backend upgrade required'):undefined" @click="discover"><Icon name="refresh" :size="14" />{{ discovering?t('Loading models…'):t('Load models from endpoint') }}</button></div>
          <div v-if="modelError" class="inline-error" role="alert">{{ modelError }}<p>{{ t('You can still enter a model ID or alias manually.') }}</p></div>
          <p v-if="models" class="form-help">{{ t('{count} models returned', {count:models.models.length}) }} · <span>{{ models.source_url }}</span><br v-if="models.has_more" /><span v-if="models.has_more">{{ t('The endpoint has more models; this is the first page.') }}</span></p>
          <label>{{ t('Default model') }}<ModelPicker v-model="form.model" :models="choices" :placeholder="t('Choose a model or enter an ID')" /></label>
          <p class="form-help">{{ t('Native Codex uses its client catalog. Custom endpoints use the models API and may require an API key. No inference is requested.') }}</p>
          <label v-if="effortOptions.length">{{ t('Thinking depth') }}<select v-model="form.effort"><option value="">{{ t('Automatic · provider default') }}</option><option v-for="effort in effortOptions" :key="effort" :value="effort">{{ effortLabel(effort) }}</option></select></label>
          <p v-else class="form-help">{{ t('Load a model list to show the reasoning levels supported by this model.') }}</p>
          <label>{{ t('Model aliases') }}<textarea v-model="aliasesText" rows="3" spellcheck="false" placeholder="fast = actual-model-id&#10;review = another-model-id" /></label>
          <p v-if="aliases.error" class="inline-error">{{ t(aliases.error) }}</p>
          <p v-else class="form-help">{{ t('One alias = model-id per line. Aliases change display selection, not the provider protocol.') }}</p>
          <p v-if="form.model" class="model-effective">{{ t('Effective model') }}: <code>{{ resolved }}</code></p>
          <label>{{ t('Permission intent') }}<select v-model="form.permission_mode"><option value="native">{{ t('Native · provider defaults') }}</option><option value="interactive">{{ t('Interactive · conservative native prompts') }}</option><option value="trusted">{{ t('Trusted · native file-edit trust') }}</option><option v-if="form.provider==='claude_code'" value="plan">{{ t('Plan · review before edits') }}</option><option value="blocked">{{ t('Blocked · do not start') }}</option></select></label>
          <p class="form-help">{{ t('AgentDock maps this intent to provider settings; the native client owns the actual permissions and approvals.') }}</p>
          </template>
          <EnvironmentEditor v-model="environmentDraft" :supported="backendCapabilities.environment" :disabled="busy" />
          <p class="form-help">{{ t('Profile environment defaults are copied into new sessions. Existing sessions keep their saved overrides; native configuration files are not edited.') }}</p>
          <p v-if="environmentError" class="inline-error" role="alert">{{ t(environmentError) }}</p>
          <p v-if="editingManaged" class="form-help">{{ t('Manage this account from Official accounts. Its profile cannot be removed separately from its account.') }}</p>
          <div class="dialog-actions"><button v-if="editing && !editingManaged" type="button" class="text-button danger-text" :disabled="busy" @click="deleteId=editing">{{ t(editingNative?'Remove profile reference':'Delete profile') }}</button><span class="flex-spacer" /><button type="button" class="secondary-button" :disabled="busy" @click="requestLeave(() => { formVisible=false; })">{{ t('Cancel') }}</button><button class="primary-button" :disabled="busy||!canSave">{{ busy?t('Saving…'):t('Save profile') }}</button></div>
        </form>
        <div v-else class="pane-empty"><Icon name="settings" :size="28" /><h3>{{ t('A profile for each context') }}</h3><p>{{ t('Select a profile to edit, or create one for a different account or endpoint.') }}</p></div>
        <div v-if="deleteId" class="confirmation-bar">{{ t(deletingProfile?.native_config?'Remove this profile reference? Native settings, sign-in and history stay on the host. Existing sessions retain their reference.':'Delete this profile? Existing session snapshots remain.') }} {{ deletingProfile?.name }}<div class="toolbar-buttons"><button class="small-button danger" :disabled="busy" @click="remove">{{ t('Confirm delete') }}</button><button class="small-button" :disabled="busy" @click="deleteId=undefined">{{ t('Keep profile') }}</button></div></div>
      </div>
    </div>
  </ModalDialog>
</template>
<style scoped>
.model-fetch {display:flex;align-items:center;justify-content:space-between;gap:12px;border-top:1px solid #e7ecef;padding-top:12px;}
.model-fetch strong {font-size:12px;}.model-effective {margin:0;color:#087e73;font-size:12px;}.form-help {overflow-wrap:anywhere;}
.native-import-button{background:#eef8f4;border-color:#cfe6dd;color:#247f6e}.native-source-controls{display:flex;align-items:end;gap:8px}.native-source-controls>label{flex:1;min-width:0}.native-source-controls>.icon-button{margin-bottom:5px}.shared-config-panel{border:1px solid #d9e9e3;border-radius:8px;background:#f5fbf8;padding:12px}.shared-config-panel>strong{display:flex;align-items:center;gap:7px;font-size:11px;font-weight:550;color:#3d8071}.shared-config-panel>code{display:block;margin-top:8px;font-size:10px;overflow-wrap:anywhere;color:#567e78}.shared-config-panel p{font-size:10px;line-height:18px;color:#6e8584;margin-top:8px}.shared-config-consent{padding:12px;border:1px solid #ece4c8;border-radius:8px;background:#fffaf0}.shared-config-consent p{font-size:10px;line-height:18px;color:#837e66;margin:0 0 8px}.shared-config-consent>label{flex-direction:row;align-items:flex-start;gap:8px;line-height:17px;color:#626b5b}.shared-config-consent input{width:14px;height:14px;margin-top:2px;flex-shrink:0;accent-color:#258672}.profile-card .shared-profile-label{color:#478b7d}.profile-card .profile-path{max-width:143px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:#98aaa6}
</style>
