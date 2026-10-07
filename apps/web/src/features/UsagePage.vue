<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import type { Session, Workspace } from '@agentdock/protocol';
import { ApiError, errorMessage, json, providerLabel, request } from './api';
import PageShell from './PageShell.vue';
import ProviderIcon from './ProviderIcon.vue';
import Icon from './Icon.vue';
import { useI18n } from '../i18n';
import { formatDuration } from './system-format';
import { cacheHitRate, cacheSavings, formatCost, formatHours, formatTokens, rangeStart, trend, type Allowance, type Price, type UsageBucket, type UsageProvider, type UsageReport } from './usage-model';

/**
 * Where the tokens went, laid out as a dashboard: the subscription windows
 * first, then ten figures with their change, then when the work happened.
 * Everything comes from the clients' own logs on this host.
 */
const props = defineProps<{ sessions: Session[]; workspaces: Workspace[] }>();
const emit = defineEmits<{ back: []; openSession: [session: Session] }>();
const { t, locale } = useI18n();

const RANGES = [{ id: 'today', label: 'Today', days: 1 }, { id: '24h', label: '24H', hours: 24 }, { id: '7d', label: '7D', days: 7 }, { id: '30d', label: '30D', days: 30 }, { id: '90d', label: '90D', days: 90 }] as const;
type RangeId = typeof RANGES[number]['id'];
const range = ref<RangeId>('30d');
const filter = reactive({ provider: '' as UsageProvider | '', model: '', project: '' });
const report = ref<UsageReport>(), allowance = ref<Allowance[]>([]), loading = ref(false), error = ref(''), unsupported = ref(false);

async function load() {
  loading.value = true; error.value = '';
  const now = Math.floor(Date.now() / 1000);
  const chosen = RANGES.find(entry => entry.id === range.value)!;
  const from = 'hours' in chosen ? now - chosen.hours * 3600 : rangeStart(chosen.days);
  const query = new URLSearchParams({ from: String(from), to: String(now + 60), tz: String(-new Date().getTimezoneOffset()) });
  for (const [key, value] of Object.entries(filter)) if (value) query.set(key, value);
  try {
    const [next, windows] = await Promise.all([
      request<UsageReport>(`/usage?${query}`),
      request<Allowance[]>('/usage/allowance').catch(() => [] as Allowance[]),
    ]);
    report.value = next; allowance.value = windows;
  } catch (cause) {
    if (cause instanceof ApiError && [404, 501].includes(cause.status)) unsupported.value = true;
    else error.value = errorMessage(cause);
  } finally { loading.value = false; }
}
onMounted(load);
watch([range, () => ({ ...filter })], load, { deep: true });

/** One card per account, its windows side by side. */
const accounts = computed(() => {
  const byId = new Map<string, { id: string; name: string; provider: UsageProvider; plan: string | null; windows: Allowance[] }>();
  for (const row of allowance.value) {
    const entry = byId.get(row.account_id) ?? { id: row.account_id, name: row.account, provider: row.provider, plan: row.plan, windows: [] };
    entry.windows.push(row); byId.set(row.account_id, entry);
  }
  return [...byId.values()].map(entry => ({ ...entry, windows: entry.windows.sort((a, b) => a.window_minutes - b.window_minutes) }));
});
const windowShort = (row: Allowance) => row.window_minutes >= 7 * 24 * 60 - 60 ? '7d' : row.window_minutes <= 5 * 60 + 30 ? '5h' : `${Math.round(row.window_minutes / 60)}h`;
const level = (percent: number) => percent >= 90 ? 'danger' : percent >= 70 ? 'warn' : 'ok';
const resetText = (row: Allowance) => row.resets_at === null ? t('Reset · starts again with your next message') : t('Resets in {time}', { time: formatDuration(Math.max(0, row.resets_at - Date.now() / 1000)) });
/** Why there is no estimate of what is left: nothing used yet gives nothing to scale from. */
const noEstimate = (row: Allowance) => row.used_percent <= 0 ? t('Nothing used yet, so nothing to estimate from') : row.used_tokens === 0 ? t('No usage logged on this host in this window') : '';

/** The ten figures: what it cost, what it was made of, and how much work it was. */
const tiles = computed(() => {
  const data = report.value; if (!data) return [];
  const now = data.totals, before = data.previous;
  const cache = (bucket: UsageBucket) => bucket.cache_read + bucket.cache_write_5m + bucket.cache_write_1h;
  const messages = (bucket: UsageBucket) => bucket.calls + bucket.user_messages;
  return [
    { key: 'cost', label: 'Estimated cost', value: formatCost(now.cost), change: trend(now.cost, before.cost), tone: 'cost', hint: now.unpriced ? t('{tokens} tokens on unpriced models are not included', { tokens: formatTokens(now.unpriced) }) : t('At API list prices') },
    { key: 'total', label: 'Total tokens', value: formatTokens(now.total), change: trend(now.total, before.total) },
    { key: 'input', label: 'Input tokens', value: formatTokens(now.input), change: trend(now.input, before.input), hint: t('Input the cache did not serve') },
    { key: 'output', label: 'Output tokens', value: formatTokens(now.output), change: trend(now.output, before.output) },
    { key: 'cache', label: 'Cache tokens', value: formatTokens(cache(now)), change: trend(cache(now), cache(before)), tone: 'quiet', hint: t('{rate}% hit rate, about {cost} saved', { rate: Math.round(cacheHitRate(now)), cost: formatCost(cacheSavings(data.models)) }) },
    { key: 'active', label: 'Active time', value: formatHours(now.active_minutes), change: trend(now.active_minutes, before.active_minutes), tone: 'time', hint: t('Five-minute stretches with any activity') },
    { key: 'duration', label: 'Total duration', value: formatHours(data.duration_seconds / 60), change: trend(data.duration_seconds, data.previous_duration_seconds), hint: t('Each session from its first event to its last') },
    { key: 'sessions', label: 'Sessions', value: data.conversation_count.toLocaleString(), change: undefined, hint: undefined },
    { key: 'messages', label: 'Total messages', value: messages(now).toLocaleString(), change: trend(messages(now), messages(before)), hint: t('Model replies and your messages') },
    { key: 'user', label: 'Your messages', value: now.user_messages.toLocaleString(), change: trend(now.user_messages, before.user_messages), hint: undefined },
  ];
});
const changeText = (value?: number) => value === undefined ? '' : `${value >= 0 ? '+' : ''}${value.toFixed(1)}%`;

/** Daily trend: stacked by kind, each kind switchable, in token, cost or time. */
type Metric = 'tokens' | 'cost' | 'time';
const METRICS = ['tokens', 'cost', 'time'] as const;
const metricLabel = (metric: Metric) => metric === 'tokens' ? 'Token' : metric === 'cost' ? 'Cost' : 'Time';
const dayMetric = ref<Metric>('tokens'), heatMetric = ref<Metric>('tokens');
const SERIES = [
  { key: 'output', label: 'Output', shade: 92 },
  { key: 'input', label: 'Input', shade: 52 },
  { key: 'cache', label: 'Cache', shade: 22 },
] as const;
type SeriesKey = typeof SERIES[number]['key'];
// Cache reads are most of the tokens and would flatten everything else; they start hidden.
const shown = reactive<Record<SeriesKey, boolean>>({ output: true, input: true, cache: false });
const seriesValue = (bucket: UsageBucket, key: SeriesKey) => key === 'output' ? bucket.output : key === 'input' ? bucket.input : bucket.cache_read + bucket.cache_write_5m + bucket.cache_write_1h;
const shade = (percent: number) => `color-mix(in srgb, var(--ink) ${percent}%, transparent)`;
/** Drawn at the width it is shown at, so its labels stay a readable size. */
const chart = reactive({ width: 640, height: 200 });
const chartBox = ref<HTMLElement>();
let resize: ResizeObserver | undefined;
onMounted(() => {
  if (typeof ResizeObserver === 'undefined') return;
  resize = new ResizeObserver(entries => { const width = entries[0]?.contentRect.width ?? 0; if (width > 120) chart.width = Math.round(width - 60); });
  watch(chartBox, (element, previous) => { if (previous) resize?.unobserve(previous); if (element) resize?.observe(element); }, { immediate: true });
});
onBeforeUnmount(() => resize?.disconnect());
const days = computed(() => report.value?.days ?? []);
interface Part { key: string; label: string; value: number; color: string }
function stack(day: UsageBucket): Part[] {
  if (dayMetric.value === 'cost') return [{ key: 'cost', label: 'Cost', value: day.cost, color: 'var(--ok)' }];
  if (dayMetric.value === 'time') return [{ key: 'time', label: 'Active time', value: day.active_minutes, color: 'var(--info)' }];
  return SERIES.filter(series => shown[series.key]).map(series => ({ key: series.key, label: series.label, value: seriesValue(day, series.key), color: shade(series.shade) }));
}
const dayTotal = (day: UsageBucket) => stack(day).reduce((sum, part) => sum + part.value, 0);
const dayMax = computed(() => Math.max(1e-9, ...days.value.map(dayTotal)));
const formatMetric = (value: number, metric: Metric) => metric === 'cost' ? formatCost(value) : metric === 'time' ? formatHours(value) : formatTokens(value);
function bars(index: number) {
  const day = days.value[index]!;
  const slot = chart.width / Math.max(1, days.value.length), width = Math.max(3, Math.min(22, slot - 4));
  const x = index * slot + (slot - width) / 2;
  let y = chart.height;
  return stack(day).map(part => {
    const height = part.value / dayMax.value * chart.height;
    y -= height;
    return { ...part, x, y, width, height: Math.max(0, height - (height > 3 ? 1.5 : 0)) };
  }).filter(rect => rect.height > 0);
}
const hover = ref(-1);
const hovered = computed(() => hover.value >= 0 ? days.value[hover.value] : undefined);
const dateLabel = (date: string) => { const [, month, day] = date.split('-'); return `${Number(month)}/${Number(day)}`; };
const labelEvery = computed(() => Math.ceil(days.value.length / 8));

/** When: weekday by hour, one hue from faint to full, and a key from few to many. */
const heat = computed(() => !report.value ? [] : heatMetric.value === 'cost' ? report.value.heatmap_cost : heatMetric.value === 'time' ? report.value.heatmap_active : report.value.heatmap);
const heatMax = computed(() => Math.max(1e-9, ...heat.value.flat()));
const heatColor = (value: number) => value > 0 ? shade(Math.round(14 + Math.sqrt(value / heatMax.value) * 82)) : shade(5);
const weekdays = computed(() => Array.from({ length: 7 }, (_, index) => new Intl.DateTimeFormat(locale.value, { weekday: 'short' }).format(new Date(2026, 0, 5 + index))));

const workspaceFor = (cwd: string) => props.workspaces.filter(workspace => cwd === workspace.root_path || cwd.startsWith(workspace.root_path.replace(/\/$/, '') + '/')).sort((a, b) => b.root_path.length - a.root_path.length)[0];
const projectName = (cwd: string) => workspaceFor(cwd)?.name ?? cwd.split('/').filter(Boolean).pop() ?? cwd;
const modelMax = computed(() => Math.max(1, ...(report.value?.models ?? []).map(row => row.total)));
const directoryMax = computed(() => Math.max(1, ...(report.value?.directories ?? []).map(([, total]) => total)));
function openConversation(id: string | null) { const session = id ? props.sessions.find(item => item.id === id) : undefined; if (session) emit('openSession', session); }
const when = (at: number) => new Intl.DateTimeFormat(locale.value, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(new Date(at * 1000));
function clearFilters() { filter.provider = ''; filter.model = ''; filter.project = ''; }

/** One price list: the published price, or the one you set for a gateway or plan. */
const editing = ref<string>(), draft = reactive<Price>({ input: 0, output: 0, cache_read: 0 }), savingPrice = ref(false);
function editPrice(model: string, price: Price | null) { editing.value = model; Object.assign(draft, price ?? { input: 0, output: 0, cache_read: 0 }); }
async function savePrice(model: string, remove = false) {
  savingPrice.value = true;
  try {
    const current = await request<{ models: Record<string, Price> }>('/usage/prices');
    const models = { ...current.models };
    if (remove) delete models[model.toLowerCase()]; else models[model.toLowerCase()] = { input: Number(draft.input) || 0, output: Number(draft.output) || 0, cache_read: Number(draft.cache_read) || 0 };
    await request('/usage/prices', json('PUT', { models }));
    editing.value = undefined; await load();
  } catch (cause) { error.value = errorMessage(cause); } finally { savingPrice.value = false; }
}

/** A few plain sentences on what stands out. */
const insights = computed(() => {
  const data = report.value; if (!data || !data.totals.total) return [];
  const lines: string[] = [];
  const top = data.models[0];
  if (top) lines.push(t('{model} did {share}% of the work.', { model: top.model, share: Math.round(top.total / data.totals.total * 100) }));
  const peak = [...data.days].sort((a, b) => b.total - a.total)[0];
  if (peak?.total) lines.push(t('The busiest day was {date}, with {tokens} tokens.', { date: peak.date, tokens: formatTokens(peak.total) }));
  let best = { day: 0, hour: 0, value: 0 };
  data.heatmap_active.forEach((row, day) => row.forEach((value, hour) => { if (value > best.value) best = { day, hour, value }; }));
  if (best.value) lines.push(t('You work hardest on {weekday} around {hour}:00.', { weekday: weekdays.value[best.day]!, hour: best.hour }));
  if (data.totals.user_messages) lines.push(t('Each of your messages led to {calls} model calls on average.', { calls: (data.totals.calls / data.totals.user_messages).toFixed(1) }));
  return lines;
});
</script>

<template>
  <PageShell :title="t('Usage')" :subtitle="t('Tokens and cost for Claude Code and Codex, read from this host\'s own client logs.')" @back="emit('back')">
    <template #actions><button class="secondary-button" :disabled="loading" @click="load"><Icon name="refresh" :size="14" />{{ t('Refresh') }}</button></template>
    <div class="usage">
      <p v-if="unsupported" class="inline-notice">{{ t('This backend does not report usage. Update AgentDock on the host to see it.') }}</p>
      <p v-else-if="error" class="inline-error" role="alert">{{ error }}</p>

      <section v-if="accounts.length" class="quota-row" :aria-label="t('Subscription allowance')">
        <article v-for="account in accounts" :key="account.id" class="quota-card">
          <header><ProviderIcon :provider="account.provider" :size="15" /><strong>{{ account.name }}</strong><span v-if="account.plan" class="plan">{{ account.plan }}</span></header>
          <div v-for="row in account.windows" :key="row.window" class="quota-line" :title="resetText(row)">
            <span class="quota-window">{{ windowShort(row) }}</span>
            <span :class="['quota-bar', level(row.used_percent)]"><i :style="{ width: Math.min(100, row.used_percent) + '%' }" /></span>
            <span class="quota-percent">{{ Math.round(row.used_percent) }}%</span>
            <small class="quota-left"><template v-if="row.estimated_remaining_tokens !== null">{{ t('≈ {tokens} left', { tokens: formatTokens(row.estimated_remaining_tokens) }) }}<template v-if="row.estimated_remaining_cost !== null"> · {{ formatCost(row.estimated_remaining_cost) }}</template><template v-if="row.low_confidence"> ?</template> · </template><template v-else-if="noEstimate(row)">{{ noEstimate(row) }} · </template>{{ resetText(row) }}<template v-if="row.observed_at"> · {{ t('from the session log, {time} ago', { time: formatDuration(Math.max(0, Date.now() / 1000 - row.observed_at)) }) }}</template></small>
          </div>
        </article>
      </section>

      <div class="filters">
        <div class="filter-row"><span class="filter-label"><Icon name="clock" :size="13" />{{ t('Date') }}</span>
          <button v-for="entry in RANGES" :key="entry.id" type="button" :class="['chip', { selected: range === entry.id }]" :aria-pressed="range === entry.id" @click="range = entry.id">{{ t(entry.label) }}</button>
        </div>
        <div class="filter-row"><span class="filter-label"><Icon name="search" :size="13" />{{ t('Filter') }}</span>
          <label :class="['chip select', { selected: !!filter.provider }]"><span>{{ t('Tool') }}</span><select v-model="filter.provider" :aria-label="t('Tool')"><option value="">{{ t('All') }}</option><option v-for="provider in report?.options.providers ?? []" :key="provider" :value="provider">{{ providerLabel(provider) }}</option></select></label>
          <label :class="['chip select', { selected: !!filter.model }]"><span>{{ t('Model') }}</span><select v-model="filter.model" :aria-label="t('Model')"><option value="">{{ t('All') }}</option><option v-for="model in report?.options.models ?? []" :key="model" :value="model">{{ model }}</option></select></label>
          <label :class="['chip select', { selected: !!filter.project }]"><span>{{ t('Project') }}</span><select v-model="filter.project" :aria-label="t('Project')"><option value="">{{ t('All') }}</option><option v-for="project in report?.options.projects ?? []" :key="project" :value="project">{{ projectName(project) }}</option></select></label>
          <button v-if="filter.provider || filter.model || filter.project" type="button" class="text-button" @click="clearFilters">{{ t('Clear') }}</button>
        </div>
      </div>

      <div v-if="!report && loading" class="pane-empty"><p>{{ t('Reading client logs… the first read of a long history takes a moment.') }}</p></div>
      <template v-if="report">
        <div class="tiles">
          <article v-for="tile in tiles" :key="tile.key" :class="['tile', tile.tone]" :title="tile.hint">
            <header><span>{{ t(tile.label) }}</span><small v-if="tile.change !== undefined">{{ changeText(tile.change) }}</small></header>
            <strong>{{ tile.value }}</strong>
          </article>
        </div>

        <div class="charts">
          <section class="panel">
            <header class="panel-head">
              <h2><Icon name="chart" :size="14" />{{ t('Daily trend') }}</h2>
              <div v-if="dayMetric === 'tokens'" class="legend"><button v-for="series in SERIES" :key="series.key" type="button" :class="{ off: !shown[series.key] }" :aria-pressed="shown[series.key]" @click="shown[series.key] = !shown[series.key]"><i :style="{ background: shade(series.shade) }" />{{ t(series.label) }}</button></div>
              <div :class="['segmented', { push: dayMetric !== 'tokens' }]"><button v-for="metric in METRICS" :key="metric" type="button" :class="{ selected: dayMetric === metric }" @click="dayMetric = metric">{{ t(metricLabel(metric)) }}</button></div>
            </header>
            <div ref="chartBox" class="day-chart" @mouseleave="hover = -1">
              <svg :viewBox="`-52 -10 ${chart.width + 60} ${chart.height + 32}`" role="img" font-size="11" :aria-label="t('Daily trend')">
                <text class="axis" x="-8" y="4" text-anchor="end">{{ formatMetric(dayMax, dayMetric) }}</text>
                <text class="axis" x="-8" :y="chart.height + 4" text-anchor="end">0</text>
                <line class="baseline" x1="0" :x2="chart.width" :y1="chart.height" :y2="chart.height" />
                <g v-for="(day, index) in days" :key="day.date">
                  <rect class="hit" :x="index * chart.width / days.length" y="0" :width="chart.width / days.length" :height="chart.height" @mouseenter="hover = index" />
                  <rect v-for="segment in bars(index)" :key="segment.key" :x="segment.x" :y="segment.y" :width="segment.width" :height="segment.height" :fill="segment.color" rx="2" :class="{ dim: hover >= 0 && hover !== index }" pointer-events="none" />
                  <text v-if="index % labelEvery === 0" class="axis" :x="index * chart.width / days.length + chart.width / days.length / 2" :y="chart.height + 18" text-anchor="middle">{{ dateLabel(day.date) }}</text>
                </g>
              </svg>
              <div v-if="hovered" class="tip" :style="{ left: `${(52 + (hover + 0.5) * chart.width / days.length) / (chart.width + 60) * 100}%` }">
                <strong>{{ hovered.date }}</strong>
                <span v-for="part in stack(hovered)" :key="part.key"><i :style="{ background: part.color }" />{{ t(part.label) }}<b>{{ formatMetric(part.value, dayMetric) }}</b></span>
                <span class="tip-more">{{ t('{messages} messages · {cost}', { messages: hovered.user_messages, cost: formatCost(hovered.cost) }) }}</span>
              </div>
            </div>
          </section>
          <section class="panel">
            <header class="panel-head">
              <h2><Icon name="clock" :size="14" />{{ t('Activity by hour') }}</h2>
              <div class="segmented push"><button v-for="metric in METRICS" :key="metric" type="button" :class="{ selected: heatMetric === metric }" @click="heatMetric = metric">{{ t(metricLabel(metric)) }}</button></div>
            </header>
            <div class="heatmap">
              <template v-for="(row, day) in heat" :key="day"><span class="heat-label">{{ weekdays[day] }}</span><span v-for="(value, hour) in row" :key="hour" class="heat-cell" :style="{ background: heatColor(value) }" :title="`${weekdays[day]} ${hour}:00 · ${formatMetric(value, heatMetric)}`" /></template>
              <span /><span v-for="hour in 24" :key="'h' + hour" class="heat-hour">{{ (hour - 1) % 3 === 0 ? String(hour - 1).padStart(2, '0') : '' }}</span>
            </div>
            <div class="heat-key"><span>{{ t('Less') }}</span><i v-for="step in [5, 22, 40, 58, 76, 96]" :key="step" :style="{ background: shade(step) }" /><span>{{ t('More') }}</span></div>
          </section>
        </div>

        <section v-if="insights.length" class="panel insights"><h2><Icon name="spark" :size="14" />{{ t('Analysis') }}</h2><ul><li v-for="line in insights" :key="line">{{ line }}</li></ul></section>

        <div class="lists">
          <section class="panel">
            <header class="panel-head"><h2>{{ t('Models') }}</h2><small>{{ t('Cost uses the published API price unless you set your own, for a gateway or a plan.') }}</small></header>
            <div class="table-wrap"><table class="table">
              <thead><tr><th>{{ t('Model') }}</th><th>{{ t('Tokens') }}</th><th>{{ t('Cache hit rate') }}</th><th>{{ t('Cost') }}</th><th>{{ t('Price per million (in / out / cache read)') }}</th></tr></thead>
              <tbody>
                <tr v-for="row in report.models" :key="row.model + row.provider">
                  <td><span class="name"><ProviderIcon :provider="row.provider" :size="13" />{{ row.model || t('Unknown model') }}</span><span class="share"><i :style="{ width: row.total / modelMax * 100 + '%' }" /></span></td>
                  <td>{{ formatTokens(row.total) }}</td><td>{{ Math.round(cacheHitRate(row)) }}%</td><td class="money">{{ row.price ? formatCost(row.cost) : '—' }}</td>
                  <td class="price">
                    <template v-if="editing === row.model"><input v-model.number="draft.input" type="number" min="0" step="0.01" :aria-label="t('Input price')" /><input v-model.number="draft.output" type="number" min="0" step="0.01" :aria-label="t('Output price')" /><input v-model.number="draft.cache_read" type="number" min="0" step="0.01" :aria-label="t('Cache read price')" /><button class="small-button primary" :disabled="savingPrice" @click="savePrice(row.model)">{{ t('Save') }}</button><button class="small-button" @click="editing = undefined">{{ t('Cancel') }}</button></template>
                    <template v-else><span v-if="row.price">${{ row.price.input }} / ${{ row.price.output }} / ${{ row.price.cache_read }}<small v-if="row.custom_price" class="mine">{{ t('yours') }}</small></span><span v-else class="muted">{{ t('No price') }}</span><button type="button" class="text-button" @click="editPrice(row.model, row.price)">{{ t(row.price ? 'Edit' : 'Set price') }}</button><button v-if="row.custom_price" type="button" class="text-button" :disabled="savingPrice" @click="savePrice(row.model, true)">{{ t('Reset') }}</button></template>
                  </td>
                </tr>
              </tbody>
            </table></div>
          </section>
          <section class="panel">
            <header class="panel-head"><h2>{{ t('Projects') }}</h2><small>{{ t('By working directory') }}</small></header>
            <ul class="projects"><li v-for="[cwd, total] in report.directories.slice(0, 8)" :key="cwd" :title="cwd"><button type="button" @click="filter.project = cwd"><strong>{{ projectName(cwd) }}</strong><small>{{ cwd }}</small></button><span class="share"><i :style="{ width: total / directoryMax * 100 + '%' }" /></span><b>{{ formatTokens(total) }}</b></li></ul>
          </section>
        </div>

        <section class="panel">
          <header class="panel-head"><h2>{{ t('Top sessions') }}</h2><small>{{ t('{shown} of {count}', { shown: Math.min(report.conversations.length, 30), count: report.conversation_count }) }}</small></header>
          <div class="table-wrap"><table class="table">
            <thead><tr><th>{{ t('Session') }}</th><th>{{ t('Models') }}</th><th>{{ t('Your messages') }}</th><th>{{ t('Tokens') }}</th><th>{{ t('Cost') }}</th><th>{{ t('Last active') }}</th></tr></thead>
            <tbody>
              <tr v-for="row in report.conversations.slice(0, 30)" :key="row.provider + row.conversation" :class="{ link: !!row.session_id && sessions.some(item => item.id === row.session_id) }" @click="openConversation(row.session_id)">
                <td><span class="name"><ProviderIcon :provider="row.provider" :size="13" /><span><strong>{{ row.title ?? projectName(row.cwd) }}</strong><small>{{ row.title ? projectName(row.cwd) : row.conversation.slice(0, 8) }}</small></span></span></td>
                <td class="muted">{{ row.models.join(', ') }}</td><td>{{ row.user_messages }}</td><td>{{ formatTokens(row.total) }}</td><td class="money">{{ row.cost ? formatCost(row.cost) : '—' }}</td><td class="muted">{{ when(row.last_at) }}</td>
              </tr>
            </tbody>
          </table></div>
        </section>
      </template>
    </div>
  </PageShell>
</template>

<style scoped>
.usage{display:flex;flex-direction:column;gap:var(--space-3);font-variant-numeric:tabular-nums}
.usage :is(h2,strong,.chip,.tile header span,th,.segmented button,.quota-window,.quota-percent){font-family:var(--mono)}
.quota-row{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:var(--space-3)}
.quota-card{display:flex;flex-direction:column;gap:8px;padding:12px 14px;border-radius:var(--radius-lg);background:var(--surface);box-shadow:var(--shadow-card)}
.quota-card header{display:flex;align-items:center;gap:8px;font-size:var(--text-sm)}
.quota-card header strong{font-weight:600;color:var(--ink);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.plan{margin-left:auto;padding:0 8px;border-radius:var(--radius-full);background:var(--fill);color:var(--ink-soft);font-size:var(--text-xs);line-height:20px}
.quota-line{display:grid;grid-template-columns:24px minmax(0,1fr) 40px;align-items:center;gap:4px 8px;font-size:var(--text-xs);color:var(--muted)}
.quota-left{grid-column:2 / 4;color:var(--muted)}
.quota-bar{height:6px;border-radius:var(--radius-full);background:var(--fill);overflow:hidden}
.quota-bar i{display:block;height:100%;border-radius:inherit;background:var(--ink-soft)}
.quota-bar.warn i{background:var(--warn)}.quota-bar.danger i{background:var(--danger)}
.quota-percent{text-align:right;color:var(--ink)}
.filters{display:flex;flex-direction:column;gap:8px}
.filter-row{display:flex;align-items:center;flex-wrap:wrap;gap:6px}
.filter-label{display:inline-flex;align-items:center;gap:5px;width:64px;font-size:var(--text-xs);color:var(--muted)}
.chip{display:inline-flex;align-items:center;gap:6px;height:28px;padding:0 11px;border:0;border-radius:var(--radius-full);background:var(--fill);color:var(--ink-soft);font-size:var(--text-xs);cursor:pointer}
.chip:hover{background:var(--fill-hover)}
.chip.selected{background:var(--ink);color:var(--surface)}
.chip.select{padding-right:4px}
.chip.select span{opacity:.75}
.chip.select select{height:26px;max-width:180px;padding:0 24px 0 0 !important;border:0;background-color:transparent;color:inherit;font:inherit;font-weight:600;background-position:right 4px center !important}
.tiles{display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:var(--space-3)}
.tile{display:flex;flex-direction:column;gap:10px;padding:14px 16px;border-radius:var(--radius-lg);background:var(--surface);box-shadow:var(--shadow-card);min-width:0}
.tile header{display:flex;justify-content:space-between;align-items:baseline;gap:8px;font-size:var(--text-xs);color:var(--muted)}
.tile header small{font-size:var(--text-xs);color:var(--muted);font-family:var(--mono)}
.tile strong{font-size:var(--text-2xl);font-weight:650;letter-spacing:-.4px;color:var(--ink);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.tile.cost strong{color:var(--ok)}
.tile.time strong{color:var(--info)}
.tile.quiet strong{color:var(--muted)}
.charts{display:grid;grid-template-columns:minmax(0,1.15fr) minmax(0,1fr);gap:var(--space-3)}
.panel{display:flex;flex-direction:column;gap:var(--space-3);padding:14px 16px;border-radius:var(--radius-lg);background:var(--surface);box-shadow:var(--shadow-card);min-width:0}
.panel h2{display:flex;align-items:center;gap:6px;font-size:var(--text-sm);font-weight:600;color:var(--ink)}
.panel-head{display:flex;align-items:center;gap:var(--space-3);flex-wrap:wrap}
.panel-head>small{margin-left:auto;font-size:var(--text-xs);color:var(--muted)}
.legend{display:flex;gap:4px;margin-left:auto}
.legend button{display:inline-flex;align-items:center;gap:5px;height:24px;padding:0 6px;border:0;border-radius:var(--radius-sm);background:none;color:var(--ink-soft);font-size:var(--text-xs);cursor:pointer}
.legend button.off{opacity:.4}
.legend i{width:8px;height:8px;border-radius:50%}
.segmented{display:inline-flex;padding:2px;border-radius:var(--radius-full);background:var(--fill)}
.segmented.push{margin-left:auto}
.segmented button{height:24px;padding:0 10px;border:0;border-radius:var(--radius-full);background:none;color:var(--muted);font-size:var(--text-xs);cursor:pointer}
.segmented button.selected{background:var(--surface);color:var(--ink);box-shadow:var(--shadow-sm)}
.day-chart{position:relative}
.day-chart svg{display:block;width:100%;height:auto;overflow:visible}
.axis{fill:var(--muted);font-family:var(--mono)}
.baseline{stroke:var(--border);stroke-width:1}
.hit{fill:transparent}
.day-chart rect.dim{opacity:.3}
.tip{position:absolute;top:0;transform:translateX(-50%);z-index:2;display:flex;flex-direction:column;gap:3px;min-width:160px;padding:8px 10px;border-radius:var(--radius-md);background:var(--surface);box-shadow:var(--shadow-lg),0 0 0 1px var(--border);font-size:var(--text-xs);color:var(--ink-soft);pointer-events:none}
.tip strong{color:var(--ink);font-size:var(--text-sm)}
.tip span{display:flex;align-items:center;gap:6px}
.tip i{width:8px;height:8px;border-radius:50%}
.tip b{margin-left:auto;color:var(--ink);font-family:var(--mono)}
.tip .tip-more{color:var(--muted);border-top:1px solid var(--border);padding-top:3px}
.heatmap{display:grid;grid-template-columns:30px repeat(24,minmax(0,1fr));gap:4px;align-items:center}
.heat-label,.heat-hour{font-size:var(--text-xs);color:var(--muted);font-family:var(--mono)}
.heat-cell{aspect-ratio:1;border-radius:var(--radius-xs)}
.heat-key{display:flex;align-items:center;justify-content:flex-end;gap:4px;font-size:var(--text-xs);color:var(--muted)}
.heat-key i{width:10px;height:10px;border-radius:var(--radius-xs)}
.insights ul{margin:0;padding-left:18px;display:flex;flex-direction:column;gap:3px;font-size:var(--text-md);color:var(--ink-soft);line-height:1.6}
.lists{display:grid;grid-template-columns:minmax(0,1.6fr) minmax(0,1fr);gap:var(--space-3);align-items:start}
.table-wrap{overflow-x:auto}
.table{width:100%;border-collapse:collapse;font-size:var(--text-sm)}
.table th{text-align:left;font-weight:500;font-size:var(--text-xs);color:var(--muted);padding:0 8px 6px;border-bottom:1px solid var(--border);white-space:nowrap}
.table td{padding:8px;border-bottom:1px solid var(--border);color:var(--ink-soft);vertical-align:middle}
.table tr:last-child td{border-bottom:0}
.table td:first-child{color:var(--ink)}
.table tr.link{cursor:pointer}
.table tr.link:hover td{background:var(--sunken)}
.money{color:var(--ok) !important;font-family:var(--mono)}
.name{display:flex;align-items:center;gap:8px;min-width:0;font-family:var(--mono)}
.name>span{display:flex;flex-direction:column;min-width:0}
.name small{font-size:var(--text-xs);color:var(--muted);font-family:var(--font-body)}
.share{display:block;height:4px;margin-top:6px;max-width:240px;border-radius:var(--radius-full);background:var(--fill);overflow:hidden}
.share i{display:block;height:100%;border-radius:inherit;background:var(--ink-soft)}
.muted{color:var(--muted)}
.price{white-space:nowrap}
.price input{width:64px;height:26px;margin-right:4px;padding:0 6px;border:1px solid var(--line);border-radius:var(--radius-sm);background:var(--surface);color:var(--ink);font-size:var(--text-sm)}
.price .text-button{margin-left:8px}
.mine{margin-left:6px;padding:0 6px;border-radius:var(--radius-full);background:var(--accent-soft);color:var(--accent-ink);font-size:var(--text-xs)}
.projects{list-style:none;margin:0;padding:0;display:flex;flex-direction:column;gap:10px}
.projects li{display:grid;grid-template-columns:minmax(0,1.3fr) minmax(0,1fr) 54px;align-items:center;gap:var(--space-3);font-size:var(--text-sm)}
.projects button{display:flex;flex-direction:column;min-width:0;padding:0;border:0;background:none;text-align:left;cursor:pointer;color:inherit}
.projects strong{font-weight:550;color:var(--ink);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.projects button:hover strong{color:var(--accent-ink)}
.projects small{font-size:var(--text-xs);color:var(--muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.projects .share{margin-top:0;max-width:none}
.projects b{text-align:right;font-weight:600;color:var(--ink);font-family:var(--mono)}
@media (max-width:1100px){.tiles{grid-template-columns:repeat(3,minmax(0,1fr))}.charts,.lists{grid-template-columns:minmax(0,1fr)}}
@media (max-width:760px){.tiles{grid-template-columns:repeat(2,minmax(0,1fr))}.filter-label{width:auto}.table th:nth-child(n+5),.table td:nth-child(n+5){display:none}}
</style>
