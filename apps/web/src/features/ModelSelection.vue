<script setup lang="ts">
import { computed, reactive, ref } from "vue";
import { useI18n } from "../i18n";
import { filterModels } from "./model-choices";
import { validWindow } from "./model-limits";

/**
 * Which of an endpoint's models a profile offers, and per model the advanced
 * settings (its context window). A gateway can serve hundreds; a session's
 * menu should hold the few someone uses. Choosing none offers them all, as
 * before. Models chosen earlier stay listed even when the endpoint's list has
 * not been loaded, so a choice can be undone without asking it again.
 */
const props = defineProps<{
  catalog: readonly { id: string; name: string }[];
  selected: readonly string[];
  windowText: (model: string) => string;
  placeholder: (model: string) => string;
}>();
const emit = defineEmits<{ "update:selected": [models: string[]]; window: [model: string, text: string]; look: [model: string] }>();
const { t } = useI18n();
const query = ref("");
const open = reactive(new Set<string>());
const chosen = computed(() => new Set(props.selected));
const rows = computed(() => {
  const listed = new Set(props.catalog.map(entry => entry.id));
  return [...props.selected.filter(id => !listed.has(id)).map(id => ({ id, name: id })), ...props.catalog];
});
const shown = computed(() => filterModels(rows.value, query.value));
function toggle(id: string) {
  emit("update:selected", chosen.value.has(id) ? props.selected.filter(model => model !== id) : [...props.selected, id]);
}
function chooseShown() { emit("update:selected", [...new Set([...props.selected, ...shown.value.map(entry => entry.id)])]); }
function clearShown() { const hidden = new Set(shown.value.map(entry => entry.id)); emit("update:selected", props.selected.filter(id => !hidden.has(id))); }
function toggleAdvanced(id: string) { if (!open.delete(id)) { open.add(id); emit("look", id); } }
</script>

<template>
  <div class="model-choice">
    <div class="model-choice-head">
      <input v-model="query" type="search" autocomplete="off" spellcheck="false" :placeholder="t('Filter by name or ID')" :aria-label="t('Filter by name or ID')" />
      <span class="model-choice-count">{{ t('{chosen} of {count} chosen', { chosen: selected.length, count: rows.length }) }}</span>
      <button type="button" class="text-button" :disabled="!shown.length" @click="chooseShown">{{ t(query.trim() ? 'Choose matches' : 'Choose all') }}</button>
      <button type="button" class="text-button" :disabled="!shown.some(entry => chosen.has(entry.id))" @click="clearShown">{{ t(query.trim() ? 'Clear matches' : 'Clear') }}</button>
    </div>
    <ul class="model-choice-list">
      <li v-for="entry in shown" :key="entry.id" :class="{ chosen: chosen.has(entry.id) }">
        <div class="model-choice-row">
          <label><input type="checkbox" :checked="chosen.has(entry.id)" @change="toggle(entry.id)" /><span class="model-choice-name">{{ entry.name }}</span><code v-if="entry.id.toLowerCase() !== entry.name.toLowerCase()">{{ entry.id }}</code></label>
          <button v-if="chosen.has(entry.id)" type="button" class="text-button" :aria-expanded="open.has(entry.id)" @click="toggleAdvanced(entry.id)">{{ t(open.has(entry.id) ? 'Hide' : 'Advanced') }}</button>
        </div>
        <div v-if="chosen.has(entry.id) && open.has(entry.id)" class="model-choice-advanced">
          <label>{{ t('Context window') }}<input :value="windowText(entry.id)" inputmode="numeric" autocomplete="off" spellcheck="false" :placeholder="placeholder(entry.id)" @input="emit('window', entry.id, ($event.target as HTMLInputElement).value)" /></label>
          <p v-if="!validWindow(windowText(entry.id))" class="inline-error">{{ t('A context window is between 1,000 and 100,000,000 tokens.') }}</p>
        </div>
      </li>
      <li v-if="!shown.length" class="model-choice-empty">{{ t('No model matches.') }}</li>
    </ul>
    <p class="form-help">{{ selected.length ? t("A session's model menu offers only the chosen models.") : t("None chosen: a session's model menu offers every model the endpoint serves.") }}</p>
  </div>
</template>

<style scoped>
.model-choice{display:flex;flex-direction:column;gap:8px;min-width:0}
.model-choice-head{display:flex;align-items:center;flex-wrap:wrap;gap:6px 12px}
.model-choice-head input{flex:1 1 180px;min-width:0}
.model-choice-count{font-size:var(--text-xs);color:var(--muted);font-variant-numeric:tabular-nums}
.model-choice-list{list-style:none;margin:0;padding:4px;max-height:280px;overflow:auto;border:1px solid var(--border);border-radius:var(--radius-md);background:var(--surface)}
.model-choice-list li{border-radius:var(--radius-sm)}
.model-choice-list li.chosen{background:var(--accent-soft)}
.model-choice-row{display:flex;align-items:center;gap:8px;padding:4px 8px}
.model-choice-row label{flex:1;min-width:0;display:flex;flex-direction:row;align-items:baseline;gap:8px;cursor:pointer;font-size:var(--text-sm);color:var(--ink)}
.model-choice-row input{flex-shrink:0;align-self:center;width:15px;height:15px;min-height:0;margin:0;padding:0}
.model-choice-name{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.model-choice-row code{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:var(--text-xs);color:var(--muted)}
.model-choice-advanced{padding:2px 8px 8px 32px;display:flex;flex-direction:column;gap:4px}
.model-choice-advanced label{display:flex;flex-direction:column;gap:4px;font-size:var(--text-xs);color:var(--muted)}
.model-choice-empty{padding:8px;font-size:var(--text-sm);color:var(--muted)}
</style>
