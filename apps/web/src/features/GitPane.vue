<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { GitDiff, GitFile, GitStatus } from "@agentdock/protocol";
import { errorMessage, json, request, workspacePath } from "./api";
import { isChangedFile, isStagedFile, parseUnifiedDiff, pathsForGitFiles } from "./git-model";
import { gitViewState } from "./git-view-state";
import Icon from "./Icon.vue";
import WorkspaceBranchMenu from "./WorkspaceBranchMenu.vue";
import { useI18n } from "../i18n";
const { t } = useI18n();
const props = defineProps<{ workspaceId: string; refreshToken?: number }>();
const emit = defineEmits<{ changed: [status: GitStatus]; openFile: [path: string] }>();
const status = ref<GitStatus>({ branch: null, files: [] });
const viewState = computed(() => gitViewState(props.workspaceId));
const selected = computed({ get: () => viewState.value.selected, set: value => { viewState.value.selected = value; } });
const diff = ref<GitDiff>();
const loading = ref(false), diffLoading = ref(false), busy = ref(false), error = ref(""), diffError = ref("");
const message = computed({ get: () => viewState.value.message, set: value => { viewState.value.message = value; } });
const notice = ref<{ hash: string; message: string }>();
const stagedFiles = computed(() => status.value.files.filter(isStagedFile));
const changedFiles = computed(() => status.value.files.filter(isChangedFile));
const groups = computed(() => [{ title: "Staged changes", staged: true, files: stagedFiles.value }, { title: "Changes", staged: false, files: changedFiles.value }]);
let revision = 0, diffRevision = 0, mutationEpoch = 0, alive = true;
const diffLines = computed(() => parseUnifiedDiff(diff.value?.diff ?? ""));
async function openDiff(path: string, staged: boolean) {
  const keepVisible = selected.value?.path === path && selected.value?.staged === staged && !!diff.value;
  selected.value = { path, staged };
  const requestId = ++diffRevision;
  diffLoading.value = !keepVisible; diffError.value = ""; if (!keepVisible) diff.value = undefined;
  try {
    const result = await request<GitDiff>(`${workspacePath(props.workspaceId)}/git/diff?path=${encodeURIComponent(path)}&staged=${staged}`);
    if (requestId === diffRevision && JSON.stringify(diff.value) !== JSON.stringify(result)) diff.value = result;
  } catch (cause) { if (requestId === diffRevision) diffError.value = errorMessage(cause); }
  finally { if (requestId === diffRevision) diffLoading.value = false; }
}
async function refresh() {
  const requestId = ++revision;
  loading.value = true; error.value = "";
  try {
    const next = await request<GitStatus>(`${workspacePath(props.workspaceId)}/git/status`);
    if (requestId !== revision) return;
    status.value = next; emit("changed", next);
    const choice = selected.value;
    const matching = choice && (choice.staged ? stagedFiles.value : changedFiles.value).some(file => file.path === choice.path);
    if (choice && matching) await openDiff(choice.path, choice.staged);
    else if (changedFiles.value[0]) await openDiff(changedFiles.value[0].path, false);
    else if (stagedFiles.value[0]) await openDiff(stagedFiles.value[0].path, true);
    else { selected.value = undefined; diff.value = undefined; diffRevision++; }
  } catch (cause) { if (requestId === revision) { status.value = { branch: null, files: [] }; selected.value = undefined; diff.value = undefined; error.value = errorMessage(cause); } }
  finally { if (requestId === revision) loading.value = false; }
}
async function stage(files: GitFile[], unstage: boolean) {
  if (!files.length || busy.value) return;
  const workspaceId = props.workspaceId, epoch = mutationEpoch;
  busy.value = true; error.value = ""; notice.value = undefined;
  try {
    const paths = pathsForGitFiles(files);
    await request(`${workspacePath(workspaceId)}/git/${unstage ? 'unstage' : 'stage'}`, json("POST", { paths }));
    if (alive && epoch === mutationEpoch) await refresh();
  } catch (cause) { if (alive && epoch === mutationEpoch) error.value = errorMessage(cause); }
  finally { if (alive && epoch === mutationEpoch) busy.value = false; }
}
/**
 * Throwing away working-tree changes cannot be undone, so it always asks
 * first, naming how many files and whether any of them are new (new files are
 * deleted, not reverted). Only the unstaged group offers it: staged work is
 * never discarded from here.
 */
const discardTarget = ref<GitFile[]>();
const discardNew = computed(() => (discardTarget.value ?? []).filter(file => file.worktree === '?').length);
async function discard() {
  const files = discardTarget.value;
  if (!files?.length || busy.value) return;
  const workspaceId = props.workspaceId, epoch = mutationEpoch;
  busy.value = true; error.value = ""; notice.value = undefined;
  try {
    await request(`${workspacePath(workspaceId)}/git/discard`, json("POST", { paths: files.map(file => file.path) }));
    discardTarget.value = undefined;
    if (alive && epoch === mutationEpoch) await refresh();
  } catch (cause) { if (alive && epoch === mutationEpoch) error.value = errorMessage(cause); }
  finally { if (alive && epoch === mutationEpoch) busy.value = false; }
}
async function commit() {
  if (!message.value.trim() || !stagedFiles.value.length || busy.value) return;
  const workspaceId = props.workspaceId, epoch = mutationEpoch, draft = viewState.value, sentMessage = message.value;
  busy.value = true; error.value = ""; notice.value = undefined;
  try {
    const result = await request<{ commit: string }>(`${workspacePath(workspaceId)}/git/commit`, json("POST", { message: sentMessage.trim() }));
    if (draft.message === sentMessage) draft.message = "";
    if (alive && epoch === mutationEpoch) { notice.value = { hash: result.commit.slice(0, 10), message: sentMessage.split('\n')[0] }; await refresh(); }
  } catch (cause) { if (alive && epoch === mutationEpoch) error.value = errorMessage(cause); }
  finally { if (alive && epoch === mutationEpoch) busy.value = false; }
}
watch(() => props.workspaceId, () => { revision++; diffRevision++; mutationEpoch++; busy.value = false; status.value = { branch: null, files: [] }; notice.value = undefined; diff.value = undefined; void refresh(); }, { immediate: true, flush: "sync" });
watch(() => props.refreshToken, () => { if (!busy.value) void refresh(); });
onBeforeUnmount(() => { alive = false; revision++; diffRevision++; mutationEpoch++; });
</script>
<template>
  <section class="git-pane">
    <div class="content-toolbar"><span class="git-branch-bar"><WorkspaceBranchMenu :workspace-id="workspaceId" :branch="status.branch" @switched="refresh" @changed="refresh" /><span class="count-badge">{{ status.files.length }}</span></span><button class="icon-button" :disabled="busy || loading" :aria-label="t('Refresh Git changes')" @click="refresh"><Icon name="refresh" :size="15" /></button></div>
    <div v-if="error" class="inline-error" role="alert">{{ error }}</div><div v-if="notice" class="inline-success" role="status">{{ t('Committed {hash} · {message}', notice) }}</div>
    <div class="git-content">
      <div class="git-sidebar">
        <form class="commit-form" @submit.prevent="commit"><textarea v-model="message" :aria-label="t('Commit message')" :placeholder="t('Commit message…')" rows="2" :disabled="busy" /><button class="primary-button" :disabled="busy || !message.trim() || !stagedFiles.length"><Icon name="check" :size="14" />{{ busy ? t('Working…') : t('Commit staged ({count})', { count: stagedFiles.length }) }}</button></form>
        <div v-if="discardTarget" class="confirmation-bar git-discard-confirm" role="alertdialog"><span>{{ discardTarget.length===1 ? t('Discard changes to {path}? This cannot be undone.', { path: discardTarget[0].path }) : t('Discard changes to {count} files? This cannot be undone.', { count: discardTarget.length }) }}<template v-if="discardNew"> {{ t('{count} new files will be deleted.', { count: discardNew }) }}</template></span><div class="toolbar-buttons"><button type="button" class="small-button danger" :disabled="busy" @click="discard">{{ t(busy ? 'Working…' : 'Discard') }}</button><button type="button" class="small-button" :disabled="busy" @click="discardTarget=undefined">{{ t('Cancel') }}</button></div></div>
        <div class="git-groups"><section v-for="group in groups" :key="group.title" class="git-group"><header><strong>{{ t(group.title) }} <span>{{ group.files.length }}</span></strong><span class="git-group-actions"><button v-if="!group.staged" class="text-button danger-text" :disabled="busy || !group.files.length" @click="discardTarget=group.files">{{ t('Discard all') }}</button><button class="text-button" :disabled="busy || !group.files.length" @click="stage(group.files, group.staged)">{{ t(group.staged ? 'Unstage all' : 'Stage all') }}</button></span></header><div v-for="file in group.files" :key="file.path" :class="['git-file', { selected: selected?.path === file.path && selected.staged === group.staged }]"><button class="git-file-open" :title="file.original_path ? `${file.original_path} → ${file.path}` : file.path" @click="openDiff(file.path, group.staged)"><span :class="['git-file-code', { staged: group.staged }]">{{ group.staged ? file.index : file.worktree }}</span><span>{{ file.path }}</span></button><button v-if="!group.staged" class="icon-button git-discard" :aria-label="t('Discard changes to {path}', { path: file.path })" :title="t('Discard changes')" :disabled="busy" @click="discardTarget=[file]">↺</button><button class="icon-button" :aria-label="t(group.staged ? 'Unstage {path}' : 'Stage {path}', { path: file.path })" :disabled="busy" @click="stage([file], group.staged)">{{ group.staged ? '−' : '+' }}</button></div><p v-if="!group.files.length" class="group-empty">{{ t(error ? 'Git unavailable' : loading ? 'Checking…' : group.staged ? 'Nothing staged' : 'Working tree clean') }}</p></section></div>
      </div>
      <div class="diff-view">
        <div v-if="selected" class="diff-title"><span class="truncate" :title="selected.path">{{ selected.path }}</span><span class="diff-badge">{{ t(selected.staged ? 'Staged' : 'Working tree') }}</span><button class="icon-button" :aria-label="t('Open changed file in editor')" @click="emit('openFile', selected.path)"><Icon name="file" :size="14" /></button></div>
        <div v-if="diffLoading" class="pane-empty"><p>{{ t('Loading diff…') }}</p></div>
        <div v-else-if="diffError" class="inline-error" role="alert">{{ diffError }}</div>
        <div v-else-if="!selected" class="pane-empty"><span class="empty-icon teal"><Icon name="check" :size="26" /></span><h3>{{ t(loading ? 'Checking changes…' : error ? 'Git unavailable' : 'All clear') }}</h3><p>{{ t(error ? 'Register a Git repository to review changes here.' : 'File changes appear here as you and your agents work.') }}</p></div>
        <template v-else-if="diff"><div v-if="diff.binary" class="inline-notice">{{ t('Binary file changed. Open it in Explorer for a preview.') }}</div><div v-if="diff.truncated" class="inline-notice">{{ t('This diff was truncated by the server output limit.') }}</div><div v-if="diff.diff" class="diff-lines" tabindex="0" :aria-label="t('Unified Git diff')"><div v-for="(line, index) in diffLines" :key="index" :class="['diff-line', line.kind]"><span class="line-number">{{ line.before }}</span><span class="line-number">{{ line.after }}</span><code>{{ line.text || ' ' }}</code></div></div><div v-else class="pane-empty"><p>{{ t('No textual diff for this change.') }}</p></div></template>
      </div>
    </div>
  </section>
</template>

<style scoped>
.git-group-actions{display:flex;align-items:center;gap:10px}
.git-discard{color:var(--muted);font-size:14px}
.git-discard:hover:not(:disabled){color:var(--danger)}
.git-discard-confirm{margin:0 0 10px}

/* The composer's chips open upward; this one sits at the top of its pane. */
.git-branch-bar{display:flex;align-items:center;gap:8px;min-width:0;font-size:12px;font-weight:600}
</style>
