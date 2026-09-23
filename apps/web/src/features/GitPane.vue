<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { GitDiff, GitFile, GitStatus } from "@agentdock/protocol";
import { errorMessage, json, request, workspacePath } from "./api";
import { isChangedFile, isStagedFile, parseUnifiedDiff, pathsForGitFiles } from "./git-model";
import { gitViewState } from "./git-view-state";
import Icon from "./Icon.vue";
import ChipMenu from "./ChipMenu.vue";
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
/**
 * Branches and worktrees, from the branch name in the toolbar.
 *
 * Switching rewrites this checkout, so the server refuses it while a session
 * here is running, and Git refuses it over uncommitted work; either refusal
 * is shown as it is. A single session moves to another branch from its own
 * branch chip, into a worktree beside this checkout; those are listed here.
 * disturbing this one: it opens as a workspace of its own.
 */
interface Worktree { path: string; branch?: string | null; main: boolean }
const branchMenu = ref<InstanceType<typeof ChipMenu>>();
const branches = ref<{ current?: string | null; branches: string[]; worktrees: Worktree[] }>();
const newBranch = ref("");
/** Every worktree except the one this pane shows. */
const otherWorktrees = computed(() => (branches.value?.worktrees ?? []).filter(tree => !(tree.branch && tree.branch === branches.value?.current)));
const worktreeFor = (branch: string) => branches.value?.worktrees.find(tree => tree.branch === branch);
async function loadBranches() {
  try { branches.value = await request(`${workspacePath(props.workspaceId)}/git/branches`); }
  catch (cause) { error.value = errorMessage(cause); }
}
async function branchAction(run: () => Promise<unknown>) {
  if (busy.value) return;
  busy.value = true; error.value = ""; notice.value = undefined;
  try { await run(); branchMenu.value?.close(true); newBranch.value = ""; await refresh(); }
  catch (cause) { error.value = errorMessage(cause); branchMenu.value?.close(true); }
  finally { busy.value = false; }
}
const switchTo = (branch: string, create = false) => branchAction(() => request(`${workspacePath(props.workspaceId)}/git/switch`, json("POST", { branch, create })));
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
    <div class="content-toolbar"><span class="git-branch-bar"><ChipMenu ref="branchMenu" class="git-branch" :label="status.branch || t('Detached HEAD')" :title="t('Branches and worktrees')" :disabled="busy" @toggle="(event: Event) => { if ((event.target as HTMLDetailsElement).open) void loadBranches(); }"><template #mark><Icon name="git" :size="13" /></template>
          <div class="git-branch-panel">
            <p v-if="!branches" class="git-branch-note">{{ t('Loading…') }}</p>
            <template v-else>
              <header>{{ t('Branches') }}</header>
              <div v-for="name in branches.branches" :key="name" :class="['git-branch-row',{current:name===branches.current}]">
                <button type="button" :disabled="busy||name===branches.current||!!worktreeFor(name)&&!worktreeFor(name)!.main" :title="worktreeFor(name)&&!worktreeFor(name)!.main?t('Checked out in another worktree'):t('Switch this checkout to {branch}', { branch: name })" @click="switchTo(name)"><Icon v-if="name===branches.current" name="check" :size="12" /><span>{{ name }}</span></button>
              </div>
              <form class="git-branch-new" @submit.prevent="newBranch.trim()&&switchTo(newBranch.trim(), true)"><input v-model="newBranch" :placeholder="t('New branch name')" :aria-label="t('New branch name')" maxlength="200" spellcheck="false" autocomplete="off" /><div><button type="submit" :disabled="busy||!newBranch.trim()">{{ t('Create here') }}</button></div></form>
              <template v-if="otherWorktrees.length">
                <header>{{ t('Worktrees') }}</header>
                <div v-for="tree in otherWorktrees" :key="tree.path" class="git-branch-worktree" :title="tree.path"><strong>{{ tree.branch || t('Detached HEAD') }}</strong><small>{{ tree.path }}</small></div>
              </template>
              <p class="git-branch-note">{{ t('Switching here moves every session in this checkout. To put one session on another branch, use the branch chip in its message box: it runs in its own worktree.') }}</p>
            </template>
          </div>
        </ChipMenu><span class="count-badge">{{ status.files.length }}</span></span><button class="icon-button" :disabled="busy || loading" :aria-label="t('Refresh Git changes')" @click="refresh"><Icon name="refresh" :size="15" /></button></div>
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

.git-branch-bar{display:flex;align-items:center;gap:8px;min-width:0}
.git-branch{max-width:220px}
/* The composer's chips open upward; this one sits at the top of its pane. */
.git-branch :deep(.chip-menu-panel){top:calc(100% + 6px);bottom:auto;width:320px;max-height:min(420px,60vh);overflow-y:auto;padding:6px}
.git-branch-panel header{margin:8px 8px 4px;font-size:10px;font-weight:600;letter-spacing:.3px;color:var(--muted)}
.git-branch-row{display:flex;align-items:center;gap:6px;border-radius:7px}
.git-branch-row:hover{background:var(--teal-soft)}
.git-branch-row>button:first-child{flex:1;min-width:0;display:flex;align-items:center;gap:6px;padding:7px 8px;border:0;background:none;text-align:left;font:12px ui-monospace,monospace;color:var(--ink);cursor:pointer}
.git-branch-row>button:first-child span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.git-branch-row.current>button:first-child{color:var(--teal);font-weight:600}
.git-branch-row>button:disabled{cursor:default;opacity:1}
.git-branch-tree{flex-shrink:0;border:0;background:none;padding:4px 8px;font-size:10.5px;color:var(--violet);cursor:pointer;opacity:0;border-radius:6px}
.git-branch-row:hover .git-branch-tree,.git-branch-tree:focus-visible{opacity:1}
.git-branch-tree:hover{background:#efeafb}
.git-branch-new{margin:8px 4px 4px;padding-top:8px;border-top:1px solid var(--line)}
.git-branch-new input{width:100%;height:30px;padding:0 8px;border:1px solid var(--line);border-radius:7px;font:12px ui-monospace,monospace;background:var(--surface);color:var(--ink)}
.git-branch-new>div{display:flex;gap:6px;margin-top:6px}
.git-branch-new button{flex:1;height:28px;border:1px solid var(--teal-line);border-radius:7px;background:var(--surface);font-size:11px;color:var(--teal);cursor:pointer}
.git-branch-new button+button{border-color:#dcd3f2;color:var(--violet)}
.git-branch-new button:disabled{opacity:.45;cursor:not-allowed}
.git-branch-worktree{display:block;width:100%;padding:7px 8px;border:0;border-radius:7px;background:none;text-align:left;cursor:pointer}
.git-branch-worktree:hover{background:#f5f2fc}
.git-branch-worktree strong{display:block;font:12px ui-monospace,monospace;color:var(--ink)}
.git-branch-worktree small{display:block;font-size:10px;color:var(--muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.git-branch-note{margin:8px 8px 4px;font-size:10.5px;line-height:1.5;color:var(--muted)}
@media(pointer:coarse){.git-branch-tree{opacity:1}}
</style>
