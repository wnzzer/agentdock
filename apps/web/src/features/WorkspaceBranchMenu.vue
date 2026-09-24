<script setup lang="ts">
import { nextTick, onBeforeUnmount, ref } from 'vue';
import { errorMessage, json, request, workspacePath } from './api';
import { useI18n } from '../i18n';

/**
 * The workspace directory's branch, and switching it.
 *
 * Shown where a workspace is named -- the status bar, the sidebar -- so the
 * branch every session in that directory shares can be changed without
 * finding the Git pane. One session on a branch of its own moves from its
 * own chip instead; this switches the directory, and says so.
 */
const props = defineProps<{ workspaceId: string; branch?: string | null; compact?: boolean }>();
const emit = defineEmits<{ switched: [] }>();
const { t } = useI18n();
const open = ref(false), busy = ref(false), error = ref(''), created = ref('');
const repo = ref<{ current?: string | null; branches: string[]; worktrees?: Array<{ branch?: string | null; main: boolean }> }>();
/** Git checks a branch out in one place at a time; one a worktree holds cannot come here. */
const heldElsewhere = (name: string) => !!repo.value?.worktrees?.some(tree => !tree.main && tree.branch === name);
const anchor = ref<HTMLElement>(), panel = ref<HTMLElement>();
const place = ref<Record<string, string>>({});

async function toggle() {
  if (open.value) { close(); return; }
  open.value = true; error.value = '';
  await nextTick(); position();
  try { repo.value = await request(`${workspacePath(props.workspaceId)}/git/branches`); await nextTick(); position(); }
  catch (cause) { error.value = errorMessage(cause); }
  document.addEventListener('pointerdown', outside, true);
}
function close() { open.value = false; document.removeEventListener('pointerdown', outside, true); }
function outside(event: Event) {
  const target = event.target as Node;
  if (!anchor.value?.contains(target) && !panel.value?.contains(target)) close();
}
/** Opens toward whichever side of the anchor has room: up from the status bar, down from the sidebar. */
function position() {
  const box = anchor.value?.getBoundingClientRect(); if (!box) return;
  const width = Math.min(300, window.innerWidth - 16), left = Math.max(8, Math.min(box.left, window.innerWidth - width - 8));
  const below = window.innerHeight - box.bottom, above = box.top;
  place.value = below >= 260 || below >= above
    ? { left: left + 'px', top: box.bottom + 6 + 'px', width: width + 'px', maxHeight: below - 16 + 'px' }
    : { left: left + 'px', bottom: window.innerHeight - box.top + 6 + 'px', width: width + 'px', maxHeight: above - 16 + 'px' };
}
async function switchTo(branch: string, create = false) {
  branch = branch.trim();
  if (!branch || busy.value || branch === repo.value?.current) return;
  busy.value = true; error.value = '';
  try {
    await request(`${workspacePath(props.workspaceId)}/git/switch`, json('POST', { branch, create }));
    created.value = ''; close(); emit('switched');
  } catch (cause) { error.value = errorMessage(cause); }
  finally { busy.value = false; }
}
onBeforeUnmount(close);
</script>

<template>
  <button ref="anchor" type="button" :class="['ws-branch', { compact }]" :aria-expanded="open" :title="t('Branch of the workspace directory · click to switch')" @click.stop="toggle">
    <svg viewBox="0 0 16 16" width="11" height="11" aria-hidden="true"><path d="M5 3v7M5 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4Zm0-7a2 2 0 1 0 0-.01M11 5a2 2 0 1 0 0-.01M11 7c0 2-2 3-6 3" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>
    <span>{{ branch || t('Detached HEAD') }}</span>
  </button>
  <Teleport to="body">
    <div v-if="open" ref="panel" class="ws-branch-panel" :style="place" role="dialog" :aria-label="t('Switch the workspace branch')">
      <header>{{ t('Workspace directory branch') }}</header>
      <p>{{ t('Switches the directory every session here shares. A session in its own worktree is not affected.') }}</p>
      <p v-if="error" class="ws-branch-error" role="alert">{{ error }}</p>
      <p v-if="!repo && !error" class="ws-branch-note">{{ t('Loading…') }}</p>
      <nav v-else-if="repo">
        <button v-for="name in repo.branches" :key="name" type="button" :class="{ current: name === repo.current }" :disabled="busy || heldElsewhere(name)" :title="heldElsewhere(name) ? t('Checked out in another worktree') : undefined" @click="switchTo(name)"><span>{{ name }}</span><small v-if="name === repo.current">{{ t('Current') }}</small><small v-else-if="heldElsewhere(name)">{{ t('In a worktree') }}</small></button>
      </nav>
      <form @submit.prevent="switchTo(created, true)"><input v-model="created" :placeholder="t('New branch name')" maxlength="200" spellcheck="false" autocomplete="off" :aria-label="t('New branch name')" /><button type="submit" :disabled="busy || !created.trim()">{{ t('Create') }}</button></form>
    </div>
  </Teleport>
</template>

<style scoped>
.ws-branch{display:inline-flex;align-items:center;gap:4px;min-width:0;max-width:160px;padding:1px 6px;border:1px solid transparent;border-radius:5px;background:none;color:inherit;font:inherit;cursor:pointer}
.ws-branch span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.ws-branch:hover,.ws-branch[aria-expanded=true]{background:#efeafb;color:var(--violet);border-color:#dcd3f2}
.ws-branch.compact{font-size:10px;color:var(--violet);background:#f5f2fc;max-width:110px}
.ws-branch-panel{position:fixed;z-index:1200;overflow-y:auto;padding:8px;border:1px solid var(--border);border-radius:12px;background:var(--surface);box-shadow:0 12px 32px #243b4c29;font-size:12px;color:var(--ink)}
.ws-branch-panel header{font-weight:600;margin:2px 4px}
.ws-branch-panel p{margin:4px 4px 8px;font-size:10.5px;line-height:1.5;color:var(--muted)}
.ws-branch-panel .ws-branch-error{color:var(--danger-ink);background:#fff3f5;border-radius:7px;padding:6px 8px}
.ws-branch-panel nav button{display:flex;align-items:center;justify-content:space-between;gap:8px;width:100%;padding:7px 8px;border:0;border-radius:7px;background:none;cursor:pointer;text-align:left;font:12px ui-monospace,monospace;color:var(--ink)}
.ws-branch-panel nav button:hover:not(:disabled),.ws-branch-panel nav button.current{background:var(--teal-soft)}
.ws-branch-panel nav button.current{color:var(--teal);font-weight:600;cursor:default}
.ws-branch-panel nav button:disabled:not(.current){color:var(--muted);cursor:not-allowed}
.ws-branch-panel nav small{font:10px system-ui,sans-serif;color:var(--muted)}
.ws-branch-panel form{display:flex;gap:6px;margin:8px 2px 2px;padding-top:8px;border-top:1px solid var(--line)}
.ws-branch-panel input{flex:1;min-width:0;height:30px;padding:0 8px;border:1px solid var(--line);border-radius:7px;font:12px ui-monospace,monospace;background:var(--surface);color:var(--ink)}
.ws-branch-panel form button{height:30px;padding:0 10px;border:0;border-radius:7px;background:var(--teal);color:#fff;font-size:11px;cursor:pointer}
.ws-branch-panel form button:disabled{opacity:.45}
@media(pointer:coarse){.ws-branch-panel nav button{min-height:44px}.ws-branch-panel input,.ws-branch-panel form button{height:40px}}
</style>
