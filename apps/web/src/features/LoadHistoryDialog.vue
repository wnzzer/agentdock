<script setup lang="ts">
import { computed,onBeforeUnmount,onMounted,ref,watch } from "vue";
import type { NativeHistoryItem,NativeHistorySource,Session,Workspace } from "@agentdock/protocol";
import { useI18n } from "../i18n";
import { errorMessage,json,request,workspacePath } from "./api";
import { fuzzyFilter } from "./fuzzy-search";
import { createHistorySelection } from "./session-connection-history";
import { backendCapabilities } from "./backend-capabilities";
import { parseEnvironmentRows, type EnvironmentRow } from "./environment-model";
import EnvironmentEditor from "./EnvironmentEditor.vue";
import ModalDialog from "./ModalDialog.vue";
import ProviderIcon from "./ProviderIcon.vue";
import Icon from "./Icon.vue";
const {t}=useI18n();
const props=defineProps<{workspace:Workspace}>();
const emit=defineEmits<{close:[];loaded:[session:Session]}>();
const sources=ref<NativeHistorySource[]>([]),sourceId=ref(""),items=ref<NativeHistoryItem[]>([]),query=ref(""),error=ref(""),truncated=ref(false),importing=ref(false);
const loadingSources=ref(false),loadingItems=ref(false),sourcesLoaded=ref(false);
const loading=computed(()=>loadingSources.value||loadingItems.value);
const selection=createHistorySelection();
const selected=computed(()=>selection.state.selected?.item);
const confirm=computed({get:()=>selection.state.confirmed,set:value=>{selection.state.confirmed=value;}});
const visible=computed(()=>fuzzyFilter(items.value,query.value,item=>[item.title,item.id,item.cwd,item.provider].join(" ")));
const environmentDrafts=ref(new Map<string,EnvironmentRow[]>());
const environmentScope=computed(()=>JSON.stringify([props.workspace.id,sourceId.value]));
const environmentDraft=computed({get:()=>environmentDrafts.value.get(environmentScope.value)??[],set:(rows:EnvironmentRow[])=>{environmentDrafts.value.set(environmentScope.value,rows);}});
const parsedEnvironment=computed(()=>parseEnvironmentRows(environmentDraft.value));
const unsupportedEnvironment=computed(()=>!backendCapabilities.environment&&environmentDraft.value.length>0);
const validEnvironment=computed(()=>!unsupportedEnvironment.value&&!parsedEnvironment.value.errors.length);
let revision=0,sourceRevision=0,importRevision=0,disposed=false;
async function load(){
  const own=++revision,workspaceId=props.workspace.id,source=sourceId.value;
  const current=()=>!disposed&&own===revision&&props.workspace.id===workspaceId&&sourceId.value===source;
  error.value="";items.value=[];selection.clear();truncated.value=false;loadingItems.value=false;
  if(!source)return;
  loadingItems.value=true;
  try{const result=await request<{items:NativeHistoryItem[];truncated:boolean}>(`${workspacePath(workspaceId)}/native-history?source_id=${encodeURIComponent(source)}`);if(current()){items.value=result.items;truncated.value=result.truncated;}}
  catch(cause){if(current())error.value=errorMessage(cause);}finally{if(current())loadingItems.value=false;}
}
async function loadSources(){
  const own=++sourceRevision;revision++;loadingSources.value=true;loadingItems.value=false;error.value="";
  items.value=[];selection.clear();truncated.value=false;
  try{
    const result=await request<NativeHistorySource[]>("/native-history/sources");
    if(disposed||own!==sourceRevision)return;
    sources.value=result;sourcesLoaded.value=true;
    sourceId.value=result.find(source=>source.id===sourceId.value)?.id??result.find(source=>source.available)?.id??result[0]?.id??"";
    await load();
  }catch(cause){if(!disposed&&own===sourceRevision)error.value=errorMessage(cause);}
  finally{if(!disposed&&own===sourceRevision)loadingSources.value=false;}
}
function refreshHistory(){if(!loading.value&&!importing.value)void loadSources();}
function selectHistory(item:NativeHistoryItem){
  if(!importing.value&&!loading.value)selection.select(props.workspace.id,sourceId.value,item);
}
async function importSelected(){
  const payload=selection.payload(props.workspace.id,sourceId.value);
  if(!payload||importing.value||loading.value||!validEnvironment.value)return;
  const own=++importRevision,workspaceId=props.workspace.id;
  importing.value=true;error.value="";
  try{
    const session=await request<Session>(`${workspacePath(workspaceId)}/native-history/import`,json("POST",{...payload,...(backendCapabilities.environment&&Object.keys(parsedEnvironment.value.environment).length?{environment:parsedEnvironment.value.environment}:{})}));
    if(!disposed&&own===importRevision&&props.workspace.id===workspaceId)emit("loaded",session);
  }catch(cause){if(!disposed&&own===importRevision)error.value=errorMessage(cause);}
  finally{if(!disposed&&own===importRevision)importing.value=false;}
}
watch(()=>props.workspace.id,()=>{revision++;importRevision++;selection.clear();items.value=[];query.value="";error.value="";truncated.value=false;importing.value=false;loadingItems.value=false;if(sourcesLoaded.value)void load();});
onMounted(()=>void loadSources());
onBeforeUnmount(()=>{disposed=true;revision++;sourceRevision++;importRevision++;});
</script>
<template>
  <ModalDialog :title="t('Load existing session')" wide :closable="!importing" @close="emit('close')">
    <p class="form-description">{{ t('Load Claude Code or Codex history for {workspace}.',{workspace:workspace.name}) }}</p>
    <div class="history-controls"><label>{{t('History source')}}<select v-model="sourceId" :disabled="loading||importing" @change="load"><option v-for="source in sources" :key="source.id" :value="source.id">{{source.label}} · {{source.available?source.path:t('Not found on this host')}}</option></select></label><button class="icon-button" :aria-label="t('Refresh history')" :disabled="loading||importing" @click="refreshHistory"><Icon name="refresh" :size="16"/></button></div>
    <div class="file-search"><Icon name="search" :size="14"/><input v-model="query" :placeholder="t('Fuzzy search title, ID or directory…')" :aria-label="t('Search native history')" /></div>
    <p class="form-help">{{t('Only metadata in this workspace is listed. No history is imported automatically.')}}</p>
    <div v-if="error" class="inline-error" role="alert">{{error}}</div>
    <div class="history-items" role="list">
      <button v-for="item in visible" :key="item.id" type="button" :class="['history-item',{selected:selected?.id===item.id}]" :disabled="importing||loading" @click="selectHistory(item)">
        <ProviderIcon :provider="item.provider"/><span><strong>{{item.title}}</strong><small>{{item.id}} · {{new Date(item.updated_at).toLocaleString()}}</small><small>{{item.cwd}}</small></span><small v-if="item.imported_session_id">{{t('Already loaded')}}</small>
      </button>
      <div v-if="loading" class="pane-empty"><p>{{t('Reading native history…')}}</p></div>
      <div v-else-if="!visible.length" class="pane-empty"><p>{{t('No matching history for this workspace.')}}</p></div>
    </div>
    <p v-if="truncated" class="form-help">{{t('Showing the most recent 500 sessions. Older history may not be included.')}}</p>
    <EnvironmentEditor v-model="environmentDraft" :supported="backendCapabilities.environment" :disabled="importing||loading||!sourceId" />
    <p class="form-help">{{ t('Environment drafts stay with this history source and workspace. An empty draft preserves existing overrides when history is loaded again.') }}</p>
    <p class="form-help">{{ t('Environment overrides affect only the launched process, not the native configuration files.') }}</p>
    <div v-for="message in parsedEnvironment.errors" :key="message" class="inline-error" role="alert">{{ t(message) }}</div>
    <div v-if="unsupportedEnvironment" class="inline-notice" role="alert">{{ t('Environment overrides require an updated backend. Your draft has not been discarded.') }}<button v-if="environmentDraft.length" type="button" class="text-button" :disabled="importing" @click="environmentDraft=[]">{{ t('Discard environment draft') }}</button></div>
    <div v-if="selected" class="history-consent"><strong>{{selected.title}}</strong><p>{{t('This resumes history using the original client configuration and account. It does not take over an existing terminal. Stop other writers to this session first.')}}</p><label><input type="checkbox" v-model="confirm" :disabled="importing||loading"/>{{t('Use the original client configuration to continue this session.')}}</label></div>
    <div class="dialog-actions"><button class="secondary-button" :disabled="importing" @click="emit('close')">{{t('Cancel')}}</button><button class="primary-button" :disabled="!selected||!confirm||importing||loading||!validEnvironment" @click="importSelected">{{t(importing?'Loading…':'Load and open')}}</button></div>
  </ModalDialog>
</template>
<style scoped>
.history-controls{display:flex;align-items:end;gap:10px;margin:14px 0}.history-controls>label{display:flex;flex-direction:column;gap:5px;flex:1;font-size:12px;min-width:0}.history-controls select{max-width:100%;padding:9px;border:1px solid #dce6e8;border-radius:6px;background:white}.history-items{max-height:320px;overflow:auto;margin:10px 0}.history-item{display:flex;align-items:center;width:100%;gap:10px;padding:12px;background:white;border:1px solid #e5ecee;border-radius:8px;margin-bottom:6px;text-align:left;cursor:pointer}.history-item.selected{border-color:#61b7a9;background:#f0f8f5}.history-item>span{flex:1;min-width:0}.history-item strong,.history-item small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.history-item strong{font-size:12px;color:#304e58}.history-item small{font-size:10px;color:#718c95;margin-top:4px}.history-consent{padding:12px;border-radius:7px;background:#fff9e9;font-size:12px;color:#5f6150}.history-consent label{display:flex;align-items:start;gap:7px}.history-consent input{margin:2px 0}
</style>
