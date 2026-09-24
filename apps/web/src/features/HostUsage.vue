<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import type { Session } from '@agentdock/protocol';
import { ApiError, providerLabel, request } from './api';
import ProviderIcon from './ProviderIcon.vue';
import { useI18n } from '../i18n';

/**
 * CPU and memory of the host, in the status bar, and of each running session
 * when it is opened.
 *
 * It polls only while the page is visible, and stops for good on a backend
 * that has no such route rather than asking again every few seconds.
 */
interface SessionUsage { session_id: string; cpu_percent: number; memory_bytes: number }
interface HostUsage { cpu_percent: number; cpu_count: number; memory_used_bytes: number; memory_total_bytes: number; sessions: SessionUsage[] }
const props = defineProps<{ sessions: Session[] }>();
const { t } = useI18n();
const usage = ref<HostUsage>(), open = ref(false), unsupported = ref(false);
let timer: ReturnType<typeof setTimeout> | undefined, disposed = false;

async function poll() {
  if (disposed || unsupported.value) return;
  if (document.visibilityState === 'visible') {
    try { usage.value = await request<HostUsage>('/host/resources'); }
    catch (cause) { if (cause instanceof ApiError && [404, 501].includes(cause.status)) { unsupported.value = true; return; } }
  }
  timer = setTimeout(poll, open.value ? 2000 : 4000);
}
onMounted(() => { void poll(); });
onBeforeUnmount(() => { disposed = true; if (timer) clearTimeout(timer); });

const memoryPercent = computed(() => usage.value && usage.value.memory_total_bytes ? usage.value.memory_used_bytes / usage.value.memory_total_bytes * 100 : 0);
/** Colour is for trouble only. Memory runs high as a matter of course -- the
 * system counts its cache as used -- so it takes more before it warns. */
const level = (percent: number, warn = 85, danger = 95) => percent >= danger ? 'danger' : percent >= warn ? 'warn' : 'ok';
function bytes(value: number) {
  const gb = value / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(gb >= 10 ? 0 : 1)} GB` : `${Math.round(value / 1024 ** 2)} MB`;
}
/** Per-session CPU is in cores; shown as a share of the machine like the total. */
const sessionRows = computed(() => {
  const cores = usage.value?.cpu_count || 1;
  return (usage.value?.sessions ?? []).map(entry => ({ ...entry, session: props.sessions.find(item => item.id === entry.session_id), share: entry.cpu_percent / cores }))
    .sort((a, b) => b.memory_bytes - a.memory_bytes);
});
function toggle() { open.value = !open.value; if (open.value) { if (timer) clearTimeout(timer); void poll(); } }
</script>

<template>
  <div v-if="usage&&!unsupported" class="host-usage">
    <button type="button" class="host-usage-chip" :aria-expanded="open" :title="t('CPU and memory on this host')" @click="toggle">
      <span :class="['host-meter', level(usage.cpu_percent)]"><b>CPU</b><i :style="{ '--fill': Math.min(100, usage.cpu_percent) + '%' }" />{{ Math.round(usage.cpu_percent) }}%</span>
      <span :class="['host-meter', level(memoryPercent, 92, 97)]"><b>{{ t('Mem') }}</b><i :style="{ '--fill': memoryPercent + '%' }" />{{ bytes(usage.memory_used_bytes) }}</span>
    </button>
    <div v-if="open" class="host-usage-panel" role="dialog" :aria-label="t('CPU and memory on this host')">
      <header><strong>{{ t('This host') }}</strong><small>{{ t('{count} cores', { count: usage.cpu_count }) }} · {{ bytes(usage.memory_used_bytes) }} / {{ bytes(usage.memory_total_bytes) }}</small></header>
      <p v-if="!sessionRows.length" class="host-usage-empty">{{ t('No session is running.') }}</p>
      <ul v-else>
        <li v-for="row in sessionRows" :key="row.session_id">
          <ProviderIcon v-if="row.session" :provider="row.session.provider" :size="13" />
          <span class="host-usage-name" :title="row.session?.title">{{ row.session?.title ?? (row.session ? providerLabel(row.session.provider) : row.session_id.slice(0, 8)) }}</span>
          <span class="host-usage-figure">{{ row.share < 0.5 ? '<1' : Math.round(row.share) }}%</span>
          <span class="host-usage-figure">{{ bytes(row.memory_bytes) }}</span>
        </li>
      </ul>
      <small class="host-usage-note">{{ t('Each session counts its client and everything it started.') }}</small>
    </div>
  </div>
</template>

<style scoped>
.host-usage{position:relative;display:flex}
.host-usage-chip{display:flex;align-items:center;gap:12px;padding:0 4px;border:0;background:none;color:inherit;font:inherit;cursor:pointer;border-radius:5px}
.host-usage-chip:hover{background:var(--fill)}
.host-meter{display:flex;align-items:center;gap:5px;font-variant-numeric:tabular-nums;color:var(--ink-soft)}
.host-meter b{font-weight:600;color:var(--muted)}
.host-meter i{--bar:#a9b6bf;position:relative;width:34px;height:4px;border-radius:3px;background:var(--fill);overflow:hidden}
.host-meter i::after{content:"";position:absolute;inset:0 auto 0 0;width:var(--fill);background:var(--bar);border-radius:3px;transition:width .6s ease}
.host-meter.warn i{--bar:#c88a1c}.host-meter.danger i{--bar:#c2415a}
.host-meter.danger{color:#c2415a}
.host-usage-panel{position:absolute;right:0;bottom:calc(100% + 8px);z-index:60;width:300px;padding:12px;border:1px solid var(--border);border-radius:12px;background:var(--surface);box-shadow:0 12px 32px #243b4c24;font-size:11.5px;color:var(--ink)}
.host-usage-panel header{display:flex;align-items:baseline;justify-content:space-between;gap:8px;margin-bottom:8px}
.host-usage-panel header strong{font-size:12.5px}
.host-usage-panel header small{color:var(--muted);font-size:10.5px}
.host-usage-panel ul{list-style:none;margin:0;padding:0;display:flex;flex-direction:column;gap:2px;max-height:240px;overflow:auto}
.host-usage-panel li{display:grid;grid-template-columns:14px minmax(0,1fr) 38px 56px;align-items:center;gap:8px;padding:5px 4px;border-radius:6px}
.host-usage-panel li:hover{background:var(--sunken)}
.host-usage-name{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.host-usage-figure{text-align:right;font-variant-numeric:tabular-nums;color:var(--ink-soft)}
.host-usage-empty{margin:6px 0;color:var(--muted)}
.host-usage-note{display:block;margin-top:8px;font-size:10px;color:var(--muted)}
@media(max-width:520px){.host-usage-chip{gap:8px}.host-meter i{display:none}.host-meter{white-space:nowrap}}
@media(prefers-reduced-motion:reduce){.host-meter i::after{transition:none}}
</style>
