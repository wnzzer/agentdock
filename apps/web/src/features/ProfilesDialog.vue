<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, reactive, ref, watch } from "vue";
import type { AgentProviderKind, EndpointProfile, ModelCatalog } from "@agentdock/protocol";
import { errorMessage, json, providerLabel, request } from "./api";
import { parseModelAliases, formatModelAliases } from "./endpoint-models";
import { sharesHostConfig, nativeProfileRenamePayload, nativeProfileUpdatePayload, profileEnvironmentPayload, profileEnvironmentDraftChanged } from "./native-profiles";
import { environmentRows, type EnvironmentRow } from "./environment-model";
import EnvironmentEditor from "./EnvironmentEditor.vue";
import { effortLabel, modelEfforts } from "./reasoning-effort";
import { useI18n } from "../i18n";
import { backendCapabilities } from "./backend-capabilities";
import ModalDialog from "./ModalDialog.vue";
import Icon from "./Icon.vue";
import ProviderIcon from "./ProviderIcon.vue";
import ModelPicker from "./ModelPicker.vue";
import { ownStoredSecret, referencedSecret, secretNameFor, type SecretName } from "./secret-names";
const { t } = useI18n();
const props = defineProps<{ profiles: EndpointProfile[]; embedded?: boolean }>();
const embedded = computed(() => props.embedded === true);
const emit = defineEmits<{ close: []; changed: [profile?: EndpointProfile]; accounts: [] }>();
// The settings shell owns the outer close, so it needs this page's unsaved guard.
defineExpose({ requestLeave: (action: () => void) => requestLeave(action) });
const editing = ref<string>(), deleteId = ref<string>();
const editingRecord = ref<EndpointProfile>();
const editingProfile = computed(() => props.profiles.find(profile => profile.id === editing.value) ?? editingRecord.value);
const editingNative = computed(() => editingProfile.value?.native_config);
const editingManaged = computed(() => editingNative.value?.source_id.startsWith('account:'));
const formVisible = ref(true), busy = ref(false), discovering = ref(false);
const error = ref(""), notice = ref(""), modelError = ref("");
const models = ref<ModelCatalog>(), aliasesText = ref("");
const environmentDraft = ref<EnvironmentRow[]>([]), initialEnvironment = ref<EnvironmentRow[]>([]);
const environmentDirty = computed(() => profileEnvironmentDraftChanged(environmentDraft.value, initialEnvironment.value));
const environmentError = computed(() => { try { profileEnvironmentPayload(environmentDraft.value, backendCapabilities.environment, initialEnvironment.value); return ""; } catch (cause) { return errorMessage(cause); } });
const pendingNavigation = ref<(() => void)>();
const form = reactive({ name: "", provider: "claude_code" as AgentProviderKind, endpoint_url: "", model: "", effort: "", permission_mode: "native" as EndpointProfile["permission_mode"], secret_ref: "", proxy_url: "" });
const secretValid = computed(() => secretMode.value === "stored" || !form.secret_ref || /^env:AGENTDOCK_SECRET_[A-Z0-9_]+$/.test(form.secret_ref));
/**
 * Where the endpoint's key lives. "stored": pasted here and kept by AgentDock
 * in the state directory (secrets.rs), usable at once. "env": a variable set
 * on the server by hand, as before. Either way the profile only ever holds
 * the reference, and the key itself never comes back to the browser.
 */
const secretMode = ref<"stored" | "env">("stored"), apiKey = ref("");
const storedSecrets = ref<SecretName[]>([]);
const otherReferences = computed(() => props.profiles.filter(profile => profile.id !== editing.value).map(profile => profile.secret_ref));
/** The stored key this profile uses now, shown as "saved". */
const storedKey = computed(() => { const name = referencedSecret(form.secret_ref); return name && storedSecrets.value.some(entry => entry.name === name && entry.source === "agentdock") ? name : undefined; });
async function loadSecrets() {
  if (!backendCapabilities.storedSecrets) return;
  try { storedSecrets.value = await request<SecretName[]>("/secrets"); } catch { /* The form still works with references. */ }
}
function chooseSecretMode(reference: string | null | undefined) {
  const name = referencedSecret(reference);
  secretMode.value = !backendCapabilities.storedSecrets || (name && !storedSecrets.value.some(entry => entry.name === name && entry.source === "agentdock")) ? "env" : "stored";
}
/** Store a pasted key and point the form at it. Its own key is replaced in
 * place; otherwise a fresh name, so no other profile's key is overwritten. */
async function storePastedKey() {
  const value = apiKey.value.trim();
  if (secretMode.value !== "stored" || !value) return;
  const name = ownStoredSecret(form.secret_ref, storedSecrets.value, otherReferences.value)
    ?? secretNameFor(form.name.trim() || providerLabel(form.provider), [...storedSecrets.value.map(entry => entry.name), ...otherReferences.value.map(referencedSecret).filter((item): item is string => !!item)]);
  await request<SecretName>("/secrets/" + encodeURIComponent(name), json("PUT", { value }));
  form.secret_ref = "env:" + name; apiKey.value = "";
  await loadSecrets();
}
/** A stored key no profile points at any more is deleted with it. */
async function releaseSecret(reference: string | null | undefined, remaining: Array<string | null | undefined>) {
  const name = referencedSecret(reference);
  if (!name || !storedSecrets.value.some(entry => entry.name === name && entry.source === "agentdock") || remaining.some(other => referencedSecret(other) === name)) return;
  try { await request("/secrets/" + encodeURIComponent(name), json("DELETE")); } catch { /* An orphaned key is harmless. */ }
  await loadSecrets();
}
const deletingProfile = computed(() => props.profiles.find(p => p.id === deleteId.value) ?? (editingRecord.value?.id === deleteId.value ? editingRecord.value : undefined));
const aliases = computed(() => { try { return { value: parseModelAliases(aliasesText.value), error: "" }; } catch (cause) { return { value: {} as Record<string,string>, error: errorMessage(cause) }; } });
const resolved = computed(() => aliases.value.value[form.model] ?? form.model);
const choices = computed(() => [...Object.entries(aliases.value.value).map(([id, actual]) => ({id,name:id + " → " + actual})), ...(models.value?.models ?? [])]);
const effortOptions = computed(() => modelEfforts(form.provider, resolved.value || undefined, models.value, form.effort));
const canSave = computed(() => !!form.name.trim() && !environmentError.value && (!!editingNative.value || (secretValid.value && !aliases.value.error)));
let discoveryRevision = 0;
function resetDiscovery() { discoveryRevision++; models.value = undefined; modelError.value = ""; discovering.value = false; }
watch(() => [form.provider, form.endpoint_url, form.proxy_url, form.secret_ref], resetDiscovery);
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
  resetDiscovery();editingRecord.value=p;
  editing.value=p?.id; formVisible.value=true; error.value=""; notice.value=""; deleteId.value=undefined;
  Object.assign(form,{name:p?.name??"",provider:p?.provider??"claude_code",endpoint_url:p?.endpoint_url??"",model:p?.model??"",effort:p?.effort??"",permission_mode:p?.permission_mode??"native",secret_ref:p?.secret_ref??"",proxy_url:p?.proxy_url??""});
  apiKey.value = ""; chooseSecretMode(p?.secret_ref);
  aliasesText.value=formatModelAliases(p?.model_aliases); models.value=undefined; modelError.value="";
}
async function discover() {
  if (!backendCapabilities.models || editingNative.value || discovering.value || !secretValid.value || aliases.value.error) return;
  // A pasted key is stored first: discovery resolves the reference on the
  // server, and storing it changes the reference, which resets discovery.
  if (secretMode.value === "stored" && apiKey.value.trim()) {
    try { await storePastedKey(); await nextTick(); } catch (cause) { modelError.value = errorMessage(cause); return; }
  }
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
    const previous=editingProfile.value?.secret_ref;
    await storePastedKey();
    const saved=await request<EndpointProfile>("/endpoint-profiles"+(editing.value?"/"+encodeURIComponent(editing.value):""),json(editing.value?"PATCH":"POST",payload()));
    if(previous!==saved.secret_ref)await releaseSecret(previous,[saved.secret_ref,...otherReferences.value]);
    notice.value=editingNative.value?(backendCapabilities.environment?"Profile updated. Process environment overrides were saved without changing the native configuration.":"Profile name updated. The native configuration was not changed."):editing.value?"Profile updated. Existing session snapshots are unchanged.":"Profile created. Select it when creating a session.";
    // Stay on what was just saved; the notice says it took.
    const message=notice.value;pendingNavigation.value=undefined;edit(saved);notice.value=message;emit("changed",saved);
  }catch(cause){error.value=errorMessage(cause);}finally{busy.value=false;}
}
async function remove(){
  if(!deleteId.value||busy.value)return;const id=deleteId.value,native=!!deletingProfile.value?.native_config;busy.value=true;error.value="";
  const reference=deletingProfile.value?.secret_ref;
  try{await request("/endpoint-profiles/"+encodeURIComponent(id),json("DELETE"));await releaseSecret(reference,props.profiles.filter(p=>p.id!==id).map(p=>p.secret_ref));deleteId.value=undefined;if(editing.value===id){formVisible.value=false;editing.value=undefined;editingRecord.value=undefined;}notice.value=native?"Profile reference removed. Native settings, credentials and history remain on the host.":"Profile deleted; no credentials were deleted from the host.";emit("changed");}
  catch(cause){error.value=errorMessage(cause);}finally{busy.value=false;}
}
/** One line per profile: the provider is already its icon, and the directory
 * is shown once the profile is opened. */
function profileSummary(p: EndpointProfile) {
  if (p.native_config) return sharesHostConfig(p.native_config) ? t('Host configuration · shared sign-in') : t('Isolated configuration · this account only');
  let host = '';
  try { host = p.endpoint_url ? new URL(p.endpoint_url).host : ''; } catch { host = p.endpoint_url ?? ''; }
  return [p.model || t('Native client default'), host || t('Official endpoint'), p.proxy_url ? t('Proxy configured') : ''].filter(Boolean).join(' · ');
}
/** Cancelling goes back to a profile rather than to an empty page. */
function cancelForm() {
  requestLeave(() => {
    const back = props.profiles.find(profile => profile.id === editing.value) ?? props.profiles[0];
    if (back) edit(back); else formVisible.value = false;
  });
}
// Open on something to read rather than on a blank page that asks for a click.
if (props.profiles.length) edit(props.profiles[0]);
void loadSecrets().then(() => chooseSecretMode(form.secret_ref));
onBeforeUnmount(()=>{discoveryRevision++;});
</script>

<template>
  <ModalDialog :title="t('Endpoint profiles')" wide :closable="!busy" :embedded="embedded" @close="requestLeave(() => emit('close'))">
    <div class="profiles-layout">
      <section class="profiles-list">
        <button class="secondary-button" :disabled="busy" @click="requestLeave(() => edit())"><Icon name="plus" :size="14" />{{ t('New profile') }}</button>
        <button v-for="p in profiles" :key="p.id" :class="['profile-card',{selected:editing===p.id&&formVisible}]" :disabled="busy" @click="requestLeave(() => edit(p))">
          <span :class="['provider-mark',p.provider]"><ProviderIcon :provider="p.provider" /></span>
          <span><strong>{{ p.name }}</strong><small :class="{'shared-profile-label':!!p.native_config}">{{ profileSummary(p) }}</small></span>
        </button>
        <p v-if="!profiles.length" class="small-empty">{{ t('No profiles yet. Native, isolated sessions also work without one.') }}</p>
        <p v-if="embedded" class="small-empty profiles-accounts-hint">{{ t('Already signed in to Claude Code or Codex on this host?') }} <button type="button" class="profiles-accounts-link" @click="requestLeave(() => emit('accounts'))">{{ t('Link it in Official accounts') }} →</button></p>
      </section>
      <div class="profile-details">
        <div v-if="notice" class="inline-success" role="status">{{ t(notice) }}</div>
        <div v-if="error" class="inline-error" role="alert">{{ error }}</div>
        <div v-if="pendingNavigation" class="confirmation-bar" role="alert">{{ t('Discard unsaved environment changes?') }}<div class="toolbar-buttons"><button type="button" class="small-button danger" @click="discardEnvironmentDraft">{{ t('Discard environment changes') }}</button><button type="button" class="small-button" @click="pendingNavigation=undefined">{{ t('Keep editing') }}</button></div></div>
        <form v-if="formVisible" class="form-stack" @submit.prevent="save">
          <h3>{{ editing ? t('Edit profile') : t('Create profile') }}</h3>
          <div class="form-columns">
            <label>{{ t('Name') }}<input v-model="form.name" required :disabled="busy" :placeholder="t('Personal {client}', { client: providerLabel(form.provider) })" maxlength="120" /></label>
            <label>{{ t('Provider') }}<select v-model="form.provider" :disabled="!!editing||busy"><option value="claude_code">Claude Code</option><option value="codex">Codex</option></select></label>
          </div>
          <template v-if="editingNative">
            <div class="shared-config-panel"><strong><ProviderIcon :provider="editingProfile!.provider" :size="17" />{{ sharesHostConfig(editingNative) ? t('Host configuration · shared sign-in') : t('Isolated configuration · this account only') }}</strong><code>{{ editingNative.config_dir }}</code><p v-if="sharesHostConfig(editingNative)">{{ t('Shared with the host: sign-in, model and permissions come from this directory, and anything else that edits it changes this profile too. AgentDock never copies or rewrites it.') }}</p><p v-else>{{ t('AgentDock created this directory for this account, and nothing else signs in through it.') }}</p></div>
          </template>
          <template v-else>
          <label>{{ t('Endpoint URL') }}<input v-model="form.endpoint_url" type="url" :placeholder="t('Official endpoint when empty')" autocomplete="off" /></label>
          <label>{{ t('Proxy URL') }}<input v-model="form.proxy_url" type="url" placeholder="http://127.0.0.1:7890" autocomplete="off" /></label>
          <p class="form-help">{{ t('Applied on the server. Empty uses the host network.') }}<template v-if="form.provider==='claude_code'"> {{ t('Claude Code does not support SOCKS.') }}</template></p>
          <div v-if="backendCapabilities.storedSecrets" class="secret-mode" role="radiogroup" :aria-label="t('Where the API key is kept')">
            <label><input v-model="secretMode" type="radio" value="stored" />{{ t('Save the key in AgentDock') }}</label>
            <label><input v-model="secretMode" type="radio" value="env" />{{ t('Server environment variable') }}</label>
          </div>
          <template v-if="secretMode==='stored'">
            <label>{{ t('API key') }}<input v-model="apiKey" type="password" autocomplete="new-password" spellcheck="false" :placeholder="storedKey ? t('Saved · leave empty to keep it') : t('Empty for an endpoint without a key')" /></label>
            <p class="form-help">{{ storedKey ? t('Kept as {name} in the state directory, readable only by you. It never returns to the browser.', { name: storedKey }) : t('Kept in the state directory, readable only by you, and usable at once. It never returns to the browser.') }}<button v-if="storedKey" type="button" class="text-button danger-text secret-clear" @click="form.secret_ref=''">{{ t('Stop using this key') }}</button></p>
          </template>
          <template v-else>
            <label>{{ t('Secret reference') }} <small>{{ t('never an API key') }}</small><input v-model="form.secret_ref" placeholder="env:AGENTDOCK_SECRET_PROVIDER_KEY" autocomplete="off" spellcheck="false" /></label>
            <p :class="secretValid?'form-help':'inline-error'">{{ secretValid?t('Name a server environment variable such as env:AGENTDOCK_SECRET_WORK. Its value never reaches the browser.'):t('Enter an environment reference, not the secret itself.') }}</p>
          </template>
          <div class="model-fetch"><strong>{{ t('Model selection') }}</strong><button type="button" class="small-button" :disabled="discovering||!backendCapabilities.models||!secretValid||!!aliases.error" :title="!backendCapabilities.models?t('Backend upgrade required'):t('Reads the endpoint\'s model list. No model is run.')" @click="discover"><Icon name="refresh" :size="14" />{{ discovering?t('Loading models…'):t('Load models from endpoint') }}</button></div>
          <div v-if="modelError" class="inline-error" role="alert">{{ modelError }}<p>{{ t('You can still enter a model ID or alias manually.') }}</p></div>
          <p v-if="models" class="form-help">{{ t('{count} models returned', {count:models.models.length}) }} · <span>{{ models.source_url }}</span><br v-if="models.has_more" /><span v-if="models.has_more">{{ t('The endpoint has more models; this is the first page.') }}</span></p>
          <label>{{ t('Default model') }}<ModelPicker v-model="form.model" :models="choices" :placeholder="t('Choose a model or enter an ID')" /></label>
          <label v-if="effortOptions.length">{{ t('Thinking depth') }}<select v-model="form.effort"><option value="">{{ t('Automatic · provider default') }}</option><option v-for="effort in effortOptions" :key="effort" :value="effort">{{ effortLabel(effort) }}</option></select></label>
          <p v-else class="form-help">{{ t('Load a model list to show the reasoning levels supported by this model.') }}</p>
          <label>{{ t('Model aliases') }}<textarea v-model="aliasesText" rows="3" spellcheck="false" placeholder="fast = actual-model-id&#10;review = another-model-id" /></label>
          <p v-if="aliases.error" class="inline-error">{{ t(aliases.error) }}</p>
          <p v-else class="form-help">{{ t('One alias = model-id per line. Aliases change display selection, not the provider protocol.') }}</p>
          <p v-if="form.model" class="model-effective">{{ t('Effective model') }}: <code>{{ resolved }}</code></p>
          <label>{{ t('Permission intent') }}<select v-model="form.permission_mode"><option value="native">{{ t('Native · provider defaults') }}</option><option value="interactive">{{ t('Interactive · conservative native prompts') }}</option><option value="trusted">{{ t('Trusted · native file-edit trust') }}</option><option v-if="form.provider==='claude_code'" value="plan">{{ t('Plan · review before edits') }}</option><option value="blocked">{{ t('Blocked · do not start') }}</option></select></label>
          </template>
          <EnvironmentEditor v-model="environmentDraft" :supported="backendCapabilities.environment" :disabled="busy" />
          <p v-if="environmentError" class="inline-error" role="alert">{{ t(environmentError) }}</p>
          <p v-if="editingManaged" class="form-help">{{ t('Manage this account from Official accounts. Its profile cannot be removed separately from its account.') }}</p>
          <div class="dialog-actions"><button v-if="editing && !editingManaged" type="button" class="text-button danger-text" :disabled="busy" @click="deleteId=editing">{{ t(editingNative?'Remove profile reference':'Delete profile') }}</button><span class="flex-spacer" /><button type="button" class="secondary-button" :disabled="busy" @click="cancelForm">{{ t('Cancel') }}</button><button class="primary-button" :disabled="busy||!canSave">{{ busy?t('Saving…'):t('Save profile') }}</button></div>
        </form>
        <div v-else class="pane-empty"><p>{{ t('Select a profile to edit, or create one for a different account or endpoint.') }}</p></div>
        <div v-if="deleteId" class="confirmation-bar">{{ t(deletingProfile?.native_config?'Remove this profile reference? Native settings, sign-in and history stay on the host. Existing sessions retain their reference.':'Delete this profile? Existing session snapshots remain.') }} {{ deletingProfile?.name }}<div class="toolbar-buttons"><button class="small-button danger" :disabled="busy" @click="remove">{{ t('Confirm delete') }}</button><button class="small-button" :disabled="busy" @click="deleteId=undefined">{{ t('Keep profile') }}</button></div></div>
      </div>
    </div>
  </ModalDialog>
</template>
<style scoped>
.secret-mode{display:flex;flex-wrap:wrap;gap:6px 16px;font-size:var(--text-xs);color:var(--ink-soft)}
.secret-mode>label{display:inline-flex;align-items:center;gap:6px;cursor:pointer}
.secret-clear{margin-left:8px;font-size:inherit}
.model-fetch {display:flex;align-items:center;justify-content:space-between;gap:12px;border-top:1px solid var(--fill-hover);padding-top:12px;}
.model-fetch strong {font-size:var(--text-sm);}.model-effective {margin:0;color:var(--accent-ink);font-size:var(--text-sm);}.form-help {overflow-wrap:anywhere;}
.shared-config-panel{border:1px solid var(--accent-line);border-radius:var(--radius-md);background:var(--sunken);padding:12px}.shared-config-panel>strong{display:flex;align-items:center;gap:7px;font-size:var(--text-xs);font-weight:550;color:var(--accent-ink)}.shared-config-panel>code{display:block;margin-top:8px;font-size:var(--text-xs);overflow-wrap:anywhere;color:#567e78}.shared-config-panel p{font-size:var(--text-xs);line-height:18px;color:var(--ink-soft);margin-top:8px}.shared-config-consent{padding:12px;border:1px solid var(--warn-line);border-radius:var(--radius-md);background:var(--warn-soft)}.shared-config-consent p{font-size:var(--text-xs);line-height:18px;color:#837e66;margin:0 0 8px}.shared-config-consent>label{flex-direction:row;align-items:flex-start;gap:8px;line-height:17px;color:#626b5b}.shared-config-consent input{width:14px;height:14px;margin-top:2px;flex-shrink:0;accent-color:var(--accent)}.profile-card .shared-profile-label{color:var(--accent-ink)}.profile-card .profile-path{max-width:143px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;color:var(--faint)}

/* Settings-page type scale: the shared form styles run from 8px to 11px in a
   pale grey, which read as faint rather than calm at this density. */
.profiles-layout{margin-top:0;grid-template-columns:210px minmax(0,1fr)}
.profile-card{align-items:center;padding:9px 10px;border-radius:var(--radius-md);margin-bottom:3px}
.profile-card.selected{background:var(--teal-soft);border-color:var(--teal-line)}
.profile-card strong{font-size:var(--text-md);line-height:18px;color:var(--ink);font-weight:550;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.profile-card small{font-size:var(--text-xs);line-height:15px;color:var(--muted);margin-top:1px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.profile-card>span:last-child{flex:1}
.profiles-list>.secondary-button{font-size:var(--text-sm);min-height:36px}
.profiles-accounts-hint{margin-top:14px;font-size:var(--text-xs);line-height:1.6;color:var(--muted);text-align:left}
.profiles-accounts-link{border:0;background:none;padding:0;font:inherit;color:var(--teal);cursor:pointer}
.profiles-accounts-link:hover{text-decoration:underline}
.form-stack{gap:14px}
.form-stack>h3{font-size:var(--text-base);font-weight:600;color:var(--ink)}
.form-stack label{font-size:var(--text-sm);color:var(--ink-soft)}
.form-stack label>small{font-size:var(--text-xs);color:var(--muted)}
.form-stack input,.form-stack select,.form-stack textarea{font-size:var(--text-md);color:var(--ink);border-color:var(--border);background:var(--surface);border-radius:var(--radius-md)}
.form-stack textarea{display:block;width:100%;padding:9px 11px;border:1px solid var(--border);line-height:1.6;font-family:var(--mono);font-size:var(--text-sm);resize:vertical;outline:0}
.form-stack textarea:focus{border-color:var(--ok-line)}
.form-stack textarea::placeholder{color:var(--faint)}
.form-help{font-size:var(--text-xs);line-height:1.7;color:var(--muted)}
.model-fetch strong{font-size:var(--text-md);color:var(--ink)}
.shared-config-panel>strong{font-size:var(--text-sm)}
.shared-config-panel>code{font-size:var(--text-xs)}
.shared-config-panel p,.shared-config-consent p{font-size:var(--text-sm);line-height:1.7}
.pane-empty p{font-size:var(--text-sm);color:var(--muted)}
/* The list stays put while a long form scrolls beside it. */
.profiles-list{position:sticky;top:0;align-self:start}
.form-stack input,.form-stack select{font-weight:400}
.form-stack :deep(.model-picker-input>input){width:auto;flex:1 1 0;min-width:0;font-weight:400;font-size:var(--text-md);color:var(--ink);background:var(--surface);border-color:var(--border);border-radius:var(--radius-md)}
@media(max-width:760px){
  .profiles-layout{grid-template-columns:minmax(0,1fr)}
  .profiles-list{position:static;border-right:0;padding-right:0;border-bottom:1px solid var(--border);padding-bottom:10px}
}
</style>
