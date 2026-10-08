<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import type { Session } from '@agentdock/protocol';
import { ApiError, providerLabel, request } from './api';
import ProviderIcon from './ProviderIcon.vue';
import { useI18n } from '../i18n';
import { RANGES, busiestSessions, chartPaths, gigahertz, measures, nearest, niceCeiling, samples, type HostHistory, type RangeKey, type Sample } from './host-history';

/**
 * Trends behind the status bar's live figures: one small chart per measure,
 * so each keeps its own axis, sharing a cursor; and the sessions that used
 * the most CPU over the range, including ones that have since ended.
 *
 * Mounted only while the host panel is open. The server writes history once
 * a minute, so it is fetched again no more often than that.
 */
const props = defineProps<{ sessions: Session[]; cores: number; fanMaxRpm?: number; clockMaxMhz?: number }>();
const { t } = useI18n();
const range = ref<RangeKey>('1h'), history = ref<HostHistory>(), unsupported = ref(false), hover = ref<number>();
let timer: ReturnType<typeof setTimeout> | undefined, disposed = false;
const span = ref({ from: 0, to: 0 });

async function load() {
  if (timer) clearTimeout(timer);
  if (disposed || unsupported.value) return;
  if (document.visibilityState === 'visible') {
    const to = Math.floor(Date.now() / 1000), from = to - RANGES.find(item => item.key === range.value)!.seconds;
    try {
      history.value = await request<HostHistory>(`/host/resources/history?from=${from}&to=${to}`);
      span.value = { from, to };
    } catch (cause) {
      if (cause instanceof ApiError && [404, 501].includes(cause.status)) { unsupported.value = true; return; }
    }
  }
  timer = setTimeout(load, 60_000);
}
onMounted(() => { void load(); });
onBeforeUnmount(() => { disposed = true; if (timer) clearTimeout(timer); });
watch(range, () => { hover.value = undefined; void load(); });

const WIDTH = 300, HEIGHT = 36;
const gb = (bytes: number) => `${(bytes / 1024 ** 3).toFixed(1)} GB`;
const memoryTotal = computed(() => history.value?.host.at(-1)?.memory_total ?? 0);

interface Chart { key: string; label: string; points: Sample[]; top: number; axis: string; format: (value: number) => string; line: string; band: string }
const charts = computed<Chart[]>(() => {
  const data = history.value;
  if (!data) return [];
  const { from, to } = span.value;
  const build = (key: string, label: string, points: Sample[], top: number, axis: string, format: (value: number) => string): Chart =>
    ({ key, label, points, top, axis, format, ...chartPaths(points, data.resolution, { from, to, width: WIDTH, height: HEIGHT, top }) });
  const peak = (points: Sample[]) => Math.max(0, ...points.map(point => point.max));
  const list = [
    build('cpu', 'CPU', samples(data.host, measures.cpu), 100, '100%', value => `${Math.round(value)}%`),
    build('memory', t('Mem'), samples(data.host, measures.memory), 100, '100%', value => `${Math.round(value)}%${memoryTotal.value ? ` · ${gb(value / 100 * memoryTotal.value)}` : ''}`),
  ];
  const clock = samples(data.host, measures.clock);
  if (clock.length) {
    // The chip's top clock where known, so a full-speed cluster reaches the top.
    const top = Math.max(props.clockMaxMhz ?? 0, peak(clock)) || niceCeiling(peak(clock), 1000);
    list.push(build('clock', t('CPU clock'), clock, top, gigahertz(top), gigahertz));
  }
  const temperature = samples(data.host, measures.temperature);
  if (temperature.length) {
    const top = niceCeiling(peak(temperature), 50);
    list.push(build('temperature', t('Temperature'), temperature, top, `${top} °C`, value => `${Math.round(value)} °C`));
  }
  const fan = samples(data.host, measures.fan);
  if (fan.length) {
    const top = niceCeiling(Math.max(peak(fan), props.fanMaxRpm ?? 0), 1000);
    list.push(build('fan', t('Fan'), fan, top, `${top} rpm`, value => `${Math.round(value)} rpm`));
  }
  return list;
});
const empty = computed(() => !!history.value && !history.value.host.length);

/** What a chart's header reads: the hovered bucket, else the latest one and the range's peak. */
function readout(chart: Chart) {
  const resolution = history.value?.resolution ?? 5;
  if (hover.value !== undefined) {
    const point = nearest(chart.points, hover.value, resolution);
    return point ? `${chart.format(point.avg)} · ${t('peak {value}', { value: chart.format(point.max) })}` : '—';
  }
  const latest = chart.points.at(-1);
  if (!latest) return '—';
  return `${chart.format(latest.avg)} · ${t('peak {value}', { value: chart.format(Math.max(...chart.points.map(point => point.max))) })}`;
}
function summary(chart: Chart) {
  return `${chart.label}: ${readout(chart)}`;
}
const cursor = computed(() => {
  if (hover.value === undefined) return undefined;
  const { from, to } = span.value;
  return `${(hover.value - from) / Math.max(1, to - from) * 100}%`;
});
function track(event: PointerEvent) {
  const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const { from, to } = span.value;
  hover.value = from + Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)) * (to - from);
}

const clock = (seconds: number, withDate: boolean) => new Date(seconds * 1000).toLocaleString(undefined, withDate ? { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' } : { hour: '2-digit', minute: '2-digit' });
const longRange = computed(() => range.value !== '1h');
const startLabel = computed(() => clock(span.value.from, longRange.value));
const hoverLabel = computed(() => hover.value === undefined ? undefined : clock(hover.value, longRange.value));

const busiest = computed(() => busiestSessions(history.value?.sessions ?? [], props.cores).map(row => ({ ...row, session: props.sessions.find(item => item.id === row.session_id) })));
const percent = (value: number) => value < 0.5 ? '<1%' : `${Math.round(value)}%`;
</script>

<template>
  <section v-if="!unsupported" class="host-history" :aria-label="t('Trends')">
    <header>
      <strong>{{ t('Trends') }}</strong>
      <small v-if="hoverLabel" class="host-history-time">{{ hoverLabel }}</small>
      <div class="host-history-ranges" role="group" :aria-label="t('Time range')">
        <button v-for="item in RANGES" :key="item.key" type="button" :aria-pressed="range === item.key" @click="range = item.key">{{ item.key }}</button>
      </div>
    </header>
    <p v-if="empty" class="host-history-empty">{{ t('History builds up while AgentDock runs; the first points arrive within a minute.') }}</p>
    <template v-else-if="history">
      <div v-for="chart in charts" :key="chart.key" class="host-trend">
        <div class="host-trend-head"><b>{{ chart.label }}</b><span>{{ readout(chart) }}</span></div>
        <div class="host-trend-plot" @pointermove="track" @pointerleave="hover = undefined">
          <svg :viewBox="`0 0 ${WIDTH} ${HEIGHT}`" preserveAspectRatio="none" role="img" :aria-label="summary(chart)">
            <path class="host-trend-band" :d="chart.band" />
            <path class="host-trend-line" :d="chart.line" />
          </svg>
          <span class="host-trend-top">{{ chart.axis }}</span>
          <i v-if="cursor" class="host-trend-cursor" :style="{ left: cursor }" />
        </div>
      </div>
      <div class="host-trend-axis"><span>{{ startLabel }}</span><span>{{ t('now') }}</span></div>
      <template v-if="busiest.length">
        <h4>{{ t('Busiest sessions') }}</h4>
        <ul>
          <li v-for="row in busiest" :key="row.session_id">
            <ProviderIcon v-if="row.session" :provider="row.session.provider" :size="13" />
            <span v-else />
            <span class="host-history-name" :title="row.session?.title">{{ row.session?.title ?? (row.session ? providerLabel(row.session.provider) : row.session_id.slice(0, 8)) }}</span>
            <span class="host-history-figure" :title="t('Average CPU')">{{ percent(row.cpu_avg) }}</span>
            <span class="host-history-figure" :title="t('Peak CPU')">{{ percent(row.cpu_max) }}</span>
          </li>
        </ul>
        <small class="host-history-note">{{ t('Average and peak CPU over the range, as a share of the machine.') }}</small>
      </template>
    </template>
  </section>
</template>

<style scoped>
.host-history{margin-top:10px;padding-top:10px;border-top:1px solid var(--border)}
.host-history header{display:flex;align-items:center;gap:8px;margin-bottom:8px}
.host-history header strong{font-size:12px}
.host-history-time{color:var(--ink-soft);font-size:10.5px;font-variant-numeric:tabular-nums}
.host-history-ranges{display:flex;gap:2px;margin-left:auto;padding:2px;border-radius:7px;background:var(--fill)}
.host-history-ranges button{min-width:30px;padding:2px 6px;border:0;border-radius:5px;background:none;color:var(--muted);font:inherit;font-size:10.5px;cursor:pointer}
.host-history-ranges button[aria-pressed=true]{background:var(--surface);color:var(--ink);box-shadow:0 1px 2px #243b4c1f}
.host-history-empty{margin:4px 0;color:var(--muted)}
.host-trend{margin-bottom:8px}
.host-trend-head{display:flex;align-items:baseline;justify-content:space-between;gap:8px;margin-bottom:3px}
.host-trend-head b{font-weight:600;color:var(--muted);font-size:10.5px}
.host-trend-head span{color:var(--ink-soft);font-variant-numeric:tabular-nums;font-size:10.5px}
.host-trend-plot{position:relative;height:36px;border-bottom:1px solid var(--line);cursor:crosshair;touch-action:none}
.host-trend-plot svg{display:block;width:100%;height:100%;overflow:visible}
.host-trend-band{fill:var(--teal-soft)}
.host-trend-line{fill:none;stroke:var(--teal);stroke-width:1.5;stroke-linejoin:round;stroke-linecap:round;vector-effect:non-scaling-stroke}
.host-trend-top{position:absolute;top:-1px;left:0;color:var(--muted);font-size:9px;line-height:1;pointer-events:none}
.host-trend-cursor{position:absolute;top:0;bottom:0;width:1px;background:var(--muted);pointer-events:none}
.host-trend-axis{display:flex;justify-content:space-between;margin-top:-4px;color:var(--muted);font-size:9.5px;font-variant-numeric:tabular-nums}
.host-history h4{margin:12px 0 4px;font-size:11px;font-weight:600}
.host-history ul{list-style:none;margin:0;padding:0;display:flex;flex-direction:column;gap:2px}
.host-history li{display:grid;grid-template-columns:14px minmax(0,1fr) 38px 38px;align-items:center;gap:8px;padding:4px;border-radius:6px}
.host-history li:hover{background:var(--sunken)}
.host-history-name{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.host-history-figure{text-align:right;font-variant-numeric:tabular-nums;color:var(--ink-soft)}
.host-history-note{display:block;margin-top:6px;font-size:10px;color:var(--muted)}
</style>
