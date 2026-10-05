<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue';
import type { Session, Workspace } from '@agentdock/protocol';
import { ApiError, errorMessage, json, providerLabel, request } from './api';
import PageShell from './PageShell.vue';
import ProviderIcon from './ProviderIcon.vue';
import Icon from './Icon.vue';
import { useI18n } from '../i18n';
import { formatDuration } from './system-format';
import { cacheHitRate, cacheSavings, formatCost, formatTokens, rangeStart, trend, type Allowance, type Price, type UsageBucket, type UsageReport } from './usage-model';

/**
 * Where the tokens went: how many, of which kind, on which model, in which
 * session, at what hour -- and, for a subscription, how much of this window is
 * probably left. Everything comes from the clients' own logs on this host.
 */
const props = defineProps<{ sessions: Session[]; workspaces: Workspace[] }>();
const emit = defineEmits<{ back: []; openSession: [session: Session] }>();
const { t, locale } = useI18n();

const RANGES = [{ days: 1, label: 'Today' }, { days: 7, label: '7 days' }, { days: 30, label: '30 days' }, { days: 90, label: '90 days' }] as const;
const days = ref<number>(30);
const report = ref<UsageReport>(), allowance = ref<Allowance[]>([]), loading = ref(false), error = ref(''), unsupported = ref(false);
const tableView = ref(false);

async function load() {
  loading.value = true; error.value = '';
  const from = rangeStart(days.value), to = Math.floor(Date.now() / 1000) + 60;
  const tz = -new Date().getTimezoneOffset();
  try {
    const [next, windows] = await Promise.all([
      request<UsageReport>(`/usage?from=${from}&to=${to}&tz=${tz}`),
      request<Allowance[]>('/usage/allowance').catch(() => [] as Allowance[]),
    ]);
    report.value = next; allowance.value = windows;
  } catch (cause) {
    if (cause instanceof ApiError && [404, 501].includes(cause.status)) unsupported.value = true;
    else error.value = errorMessage(cause);
  } finally { loading.value = false; }
}
onMounted(load);
watch(days, load);

const totals = computed(() => report.value?.totals);
const tokenTrend = computed(() => report.value ? trend(report.value.totals.total, report.value.previous.total) : undefined);
const costTrend = computed(() => report.value ? trend(report.value.totals.cost, report.value.previous.cost) : undefined);
const hitRate = computed(() => totals.value ? cacheHitRate(totals.value) : 0);
const savings = computed(() => report.value ? cacheSavings(report.value.models) : 0);
const trendText = (value?: number) => value === undefined ? t('no earlier data') : `${value >= 0 ? '↑' : '↓'} ${Math.abs(value).toFixed(0)}% ${t('vs previous')}`;

/** Inside the cache and outside it: every input token is one of these, and output is the rest. */
const SERIES = [
  { key: 'input', label: 'Uncached input', color: 'var(--chart-1)' },
  { key: 'write', label: 'Cache write', color: 'var(--chart-2)' },
  { key: 'read', label: 'Cache read', color: 'var(--chart-3)' },
  { key: 'output', label: 'Output', color: 'var(--chart-4)' },
] as const;
type SeriesKey = typeof SERIES[number]['key'];
const part = (bucket: UsageBucket, key: SeriesKey) => key === 'input' ? bucket.input : key === 'write' ? bucket.cache_write_5m + bucket.cache_write_1h : key === 'read' ? bucket.cache_read : bucket.output;
const composition = computed(() => {
  const bucket = totals.value;
  if (!bucket || !bucket.total) return [];
  return SERIES.map(series => ({ ...series, value: part(bucket, series.key), share: part(bucket, series.key) / bucket.total * 100 }));
});

/** Daily stacked bars, on one axis from zero. */
const chart = reactive({ width: 720, height: 200, hover: -1 });
const chartDays = computed(() => report.value?.days ?? []);
const dayMax = computed(() => Math.max(1, ...chartDays.value.map(day => day.total)));
const ticks = computed(() => { const top = dayMax.value; return [0, top / 2, top]; });
function bars(index: number) {
  const day = chartDays.value[index]!;
  const slot = chart.width / Math.max(1, chartDays.value.length), width = Math.max(2, Math.min(28, slot - 3));
  const x = index * slot + (slot - width) / 2;
  let y = chart.height;
  return SERIES.map(series => {
    const height = part(day, series.key) / dayMax.value * chart.height;
    y -= height;
    return { key: series.key, color: series.color, x, y, width, height: Math.max(0, height - (height > 3 ? 2 : 0)) };
  }).filter(rect => rect.height > 0);
}
const dateLabel = (date: string) => { const [, month, day] = date.split('-'); return `${Number(month)}/${Number(day)}`; };
const labelEvery = computed(() => Math.ceil(chartDays.value.length / 8));
const hovered = computed(() => chart.hover >= 0 ? chartDays.value[chart.hover] : undefined);

/** Weekday by hour, in one hue from faint to full. */
const weekdays = computed(() => Array.from({ length: 7 }, (_, index) => new Intl.DateTimeFormat(locale.value, { weekday: 'short' }).format(new Date(2026, 0, 5 + index))));
const heatMax = computed(() => Math.max(1, ...(report.value?.heatmap ?? []).flat()));
const heatColor = (value: number) => value ? `color-mix(in srgb, var(--accent) ${Math.round(12 + Math.sqrt(value / heatMax.value) * 88)}%, var(--sunken))` : 'var(--sunken)';
const busiest = computed(() => {
  const grid = report.value?.heatmap; if (!grid) return undefined;
  let best = { day: 0, hour: 0, value: 0 };
  grid.forEach((row, day) => row.forEach((value, hour) => { if (value > best.value) best = { day, hour, value }; }));
  return best.value ? best : undefined;
});
const peakDay = computed(() => [...chartDays.value].sort((a, b) => b.total - a.total)[0]);

const modelShareMax = computed(() => Math.max(1, ...(report.value?.models ?? []).map(row => row.total)));
const workspaceFor = (cwd: string) => props.workspaces.filter(workspace => cwd === workspace.root_path || cwd.startsWith(workspace.root_path.replace(/\/$/, '') + '/')).sort((a, b) => b.root_path.length - a.root_path.length)[0];
const directoryMax = computed(() => Math.max(1, ...(report.value?.directories ?? []).map(([, total]) => total)));
function openConversation(id: string | null) { const session = id ? props.sessions.find(item => item.id === id) : undefined; if (session) emit('openSession', session); }
const when = (at: number) => new Intl.DateTimeFormat(locale.value, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(new Date(at * 1000));

/** One price list for every model: the published price, or the one you set. */
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

const windowName = (row: Allowance) => row.window_minutes >= 7 * 24 * 60 - 60 ? t('This week') : row.window_minutes <= 5 * 60 + 30 ? t('5-hour window') : t('{hours} h window', { hours: Math.round(row.window_minutes / 60) });
const resetIn = (row: Allowance) => formatDuration(Math.max(0, row.resets_at - Date.now() / 1000));

/** A few plain sentences: what stands out in this range. */
const insights = computed(() => {
  const data = report.value; if (!data || !data.totals.total) return [];
  const lines: string[] = [];
  const top = data.models[0];
  if (top) lines.push(t('{model} did {share}% of the work.', { model: top.model, share: Math.round(top.total / data.totals.total * 100) }));
  lines.push(t('The cache served {rate}% of input{saved}.', { rate: Math.round(hitRate.value), saved: savings.value > 0.5 ? t(', about {cost} less than paying full price for it', { cost: formatCost(savings.value) }) : '' }));
  if (peakDay.value?.total) lines.push(t('The busiest day was {date}, with {tokens} tokens.', { date: peakDay.value.date, tokens: formatTokens(peakDay.value.total) }));
  if (busiest.value) lines.push(t('You work hardest on {weekday} around {hour}:00.', { weekday: weekdays.value[busiest.value.day]!, hour: busiest.value.hour }));
  if (data.totals.unpriced) lines.push(t('{tokens} tokens are on models without a price; set one below to include them in the cost.', { tokens: formatTokens(data.totals.unpriced) }));
  return lines;
});
</script>

<template>
  <PageShell :title="t('Usage')" :subtitle="t('Tokens and cost for Claude Code and Codex, read from this host\'s own client logs.')" @back="emit('back')">
    <template #actions>
      <div class="usage-range" role="group" :aria-label="t('Range')"><button v-for="range in RANGES" :key="range.days" type="button" :class="{ selected: days === range.days }" :aria-pressed="days === range.days" @click="days = range.days">{{ t(range.label) }}</button></div>
      <button class="secondary-button" :disabled="loading" @click="load"><Icon name="refresh" :size="14" />{{ t('Refresh') }}</button>
    </template>
    <div class="usage-page">
      <p v-if="unsupported" class="inline-notice">{{ t('This backend does not report usage. Update AgentDock on the host to see it.') }}</p>
      <p v-else-if="error" class="inline-error" role="alert">{{ error }}</p>
      <div v-if="!report && loading" class="pane-empty"><p>{{ t('Reading client logs… the first read of a long history takes a moment.') }}</p></div>
      <template v-if="report && totals">
        <div class="usage-tiles">
          <article class="usage-tile"><span>{{ t('Tokens') }}</span><strong>{{ formatTokens(totals.total) }}</strong><small :class="{ up: (tokenTrend ?? 0) > 0 }">{{ trendText(tokenTrend) }}</small></article>
          <article class="usage-tile"><span>{{ t('API-equivalent cost') }}</span><strong>{{ formatCost(totals.cost) }}</strong><small :class="{ up: (costTrend ?? 0) > 0 }">{{ trendText(costTrend) }}</small></article>
          <article class="usage-tile"><span>{{ t('Cache hit rate') }}</span><strong>{{ Math.round(hitRate) }}<em>%</em></strong><small>{{ savings > 0.5 ? t('saved about {cost}', { cost: formatCost(savings) }) : t('of all input') }}</small></article>
          <article class="usage-tile"><span>{{ t('Sessions') }}</span><strong>{{ report.conversation_count }}</strong><small>{{ t('{calls} model calls', { calls: totals.calls.toLocaleString() }) }}</small></article>
        </div>

        <section v-if="insights.length" class="usage-card usage-insights"><h2><Icon name="spark" :size="15" />{{ t('Summary') }}</h2><ul><li v-for="line in insights" :key="line">{{ line }}</li></ul></section>

        <section v-if="allowance.length" class="usage-card">
          <header class="usage-card-head"><h2>{{ t('Subscription allowance') }}</h2><small>{{ t('Estimated from the share each account has used and what this host logged in the same window.') }}</small></header>
          <div class="allowance-grid">
            <article v-for="row in allowance" :key="row.account_id + row.window" class="allowance">
              <header><ProviderIcon :provider="row.provider" :size="14" /><strong>{{ row.account }}</strong><span>{{ windowName(row) }}</span></header>
              <div class="allowance-bar" :style="{ '--used': Math.min(100, row.used_percent) + '%' }"><i /></div>
              <dl>
                <div><dt>{{ t('Used') }}</dt><dd>{{ Math.round(row.used_percent) }}% · {{ formatTokens(row.used_tokens) }}<template v-if="row.used_cost"> · {{ formatCost(row.used_cost) }}</template></dd></div>
                <div><dt>{{ t('Likely left') }}</dt><dd><template v-if="row.estimated_remaining_tokens !== null">≈ {{ formatTokens(row.estimated_remaining_tokens) }}<template v-if="row.estimated_remaining_cost !== null"> · {{ formatCost(row.estimated_remaining_cost) }}</template></template><template v-else>—</template></dd></div>
                <div><dt>{{ t('Resets in') }}</dt><dd>{{ resetIn(row) }}</dd></div>
              </dl>
              <small v-if="row.low_confidence" class="allowance-note">{{ t('Too little used yet for a firm estimate.') }}</small>
            </article>
          </div>
          <small class="usage-footnote">{{ t('Use of the same account on other machines is not in these logs, so the estimate reads optimistic when there is any.') }}</small>
        </section>

        <section class="usage-card">
          <header class="usage-card-head"><h2>{{ t('Inside and outside the cache') }}</h2><small>{{ t('Cache reads are the cheap kind; uncached input and cache writes are full price or more.') }}</small></header>
          <div class="composition" role="img" :aria-label="composition.map(item => `${t(item.label)} ${Math.round(item.share)}%`).join(', ')"><span v-for="item in composition" :key="item.key" :style="{ flexGrow: item.value, background: item.color }" :title="`${t(item.label)} · ${formatTokens(item.value)}`" /></div>
          <ul class="legend"><li v-for="item in composition" :key="item.key"><i :style="{ background: item.color }" />{{ t(item.label) }}<strong>{{ formatTokens(item.value) }}</strong><small>{{ item.share.toFixed(1) }}%</small></li></ul>
        </section>

        <section class="usage-card">
          <header class="usage-card-head"><h2>{{ t('Tokens by day') }}</h2><button type="button" class="text-button" @click="tableView = !tableView">{{ t(tableView ? 'Show chart' : 'Show as table') }}</button></header>
          <ul class="legend"><li v-for="series in SERIES" :key="series.key"><i :style="{ background: series.color }" />{{ t(series.label) }}</li></ul>
          <div v-if="!tableView" class="day-chart" @mouseleave="chart.hover = -1">
            <svg :viewBox="`-44 -8 ${chart.width + 52} ${chart.height + 30}`" role="img" font-size="11" :aria-label="t('Tokens by day')">
              <g class="grid"><template v-for="tick in ticks" :key="tick"><line x1="0" :x2="chart.width" :y1="chart.height - tick / dayMax * chart.height" :y2="chart.height - tick / dayMax * chart.height" /><text x="-8" :y="chart.height - tick / dayMax * chart.height + 4" text-anchor="end">{{ formatTokens(tick) }}</text></template></g>
              <g v-for="(day, index) in chartDays" :key="day.date">
                <rect class="hit" :x="index * chart.width / chartDays.length" y="0" :width="chart.width / chartDays.length" :height="chart.height" @mouseenter="chart.hover = index" />
                <rect v-for="segment in bars(index)" :key="segment.key" :x="segment.x" :y="segment.y" :width="segment.width" :height="segment.height" :fill="segment.color" rx="2" :class="{ dim: chart.hover >= 0 && chart.hover !== index }" pointer-events="none" />
                <text v-if="index % labelEvery === 0" class="axis" :x="index * chart.width / chartDays.length + chart.width / chartDays.length / 2" :y="chart.height + 18" text-anchor="middle">{{ dateLabel(day.date) }}</text>
              </g>
            </svg>
            <div v-if="hovered" class="chart-tip" :style="{ left: `${(44 + (chart.hover + 0.5) * chart.width / chartDays.length) / (chart.width + 52) * 100}%` }">
              <strong>{{ hovered.date }}</strong>
              <span v-for="series in [...SERIES].reverse()" :key="series.key"><i :style="{ background: series.color }" />{{ t(series.label) }}<b>{{ formatTokens(part(hovered, series.key)) }}</b></span>
              <span class="tip-total">{{ t('Total') }}<b>{{ formatTokens(hovered.total) }}</b></span><span v-if="hovered.cost">{{ t('Cost') }}<b>{{ formatCost(hovered.cost) }}</b></span>
            </div>
          </div>
          <div v-else class="table-wrap"><table class="usage-table"><thead><tr><th>{{ t('Day') }}</th><th v-for="series in SERIES" :key="series.key">{{ t(series.label) }}</th><th>{{ t('Total') }}</th><th>{{ t('Cost') }}</th></tr></thead><tbody><tr v-for="day in [...chartDays].reverse()" :key="day.date"><td>{{ day.date }}</td><td v-for="series in SERIES" :key="series.key">{{ formatTokens(part(day, series.key)) }}</td><td>{{ formatTokens(day.total) }}</td><td>{{ day.cost ? formatCost(day.cost) : '—' }}</td></tr></tbody></table></div>
        </section>

        <div class="usage-grid">
          <section class="usage-card">
            <header class="usage-card-head"><h2>{{ t('When you work') }}</h2><small>{{ t('Tokens by weekday and hour, local time') }}</small></header>
            <div class="heatmap" role="table" :aria-label="t('When you work')">
              <template v-for="(row, day) in report.heatmap" :key="day"><span class="heat-label">{{ weekdays[day] }}</span><span v-for="(value, hour) in row" :key="hour" class="heat-cell" :style="{ background: heatColor(value) }" :title="`${weekdays[day]} ${hour}:00 · ${formatTokens(value)}`" /></template>
              <span /><span v-for="hour in 24" :key="'h' + hour" class="heat-hour">{{ (hour - 1) % 6 === 0 ? hour - 1 : '' }}</span>
            </div>
          </section>
          <section class="usage-card">
            <header class="usage-card-head"><h2>{{ t('Projects') }}</h2><small>{{ t('By working directory') }}</small></header>
            <ul class="share-list"><li v-for="[cwd, total] in report.directories.slice(0, 8)" :key="cwd"><span class="share-name" :title="cwd"><strong>{{ workspaceFor(cwd)?.name ?? cwd.split('/').filter(Boolean).pop() ?? cwd }}</strong><small>{{ cwd }}</small></span><span class="share-bar"><i :style="{ width: total / directoryMax * 100 + '%' }" /></span><b>{{ formatTokens(total) }}</b></li></ul>
          </section>
        </div>

        <section class="usage-card">
          <header class="usage-card-head"><h2>{{ t('Models') }}</h2><small>{{ t('Cost uses the published API price unless you set your own, for a gateway or a plan.') }}</small></header>
          <div class="table-wrap"><table class="usage-table models">
            <thead><tr><th>{{ t('Model') }}</th><th>{{ t('Tokens') }}</th><th>{{ t('Cache hit rate') }}</th><th>{{ t('Cost') }}</th><th>{{ t('Price per million (in / out / cache read)') }}</th></tr></thead>
            <tbody>
              <tr v-for="row in report.models" :key="row.model + row.provider">
                <td><span class="model-cell"><ProviderIcon :provider="row.provider" :size="14" /><span>{{ row.model || t('Unknown model') }}</span></span><span class="share-bar inline"><i :style="{ width: row.total / modelShareMax * 100 + '%' }" /></span></td>
                <td>{{ formatTokens(row.total) }}</td>
                <td>{{ Math.round(cacheHitRate(row)) }}%</td>
                <td>{{ row.price ? formatCost(row.cost) : '—' }}</td>
                <td class="price-cell">
                  <template v-if="editing === row.model">
                    <input v-model.number="draft.input" type="number" min="0" step="0.01" :aria-label="t('Input price')" /><input v-model.number="draft.output" type="number" min="0" step="0.01" :aria-label="t('Output price')" /><input v-model.number="draft.cache_read" type="number" min="0" step="0.01" :aria-label="t('Cache read price')" />
                    <button class="small-button primary" :disabled="savingPrice" @click="savePrice(row.model)">{{ t('Save') }}</button><button class="small-button" @click="editing = undefined">{{ t('Cancel') }}</button>
                  </template>
                  <template v-else>
                    <span v-if="row.price">${{ row.price.input }} / ${{ row.price.output }} / ${{ row.price.cache_read }}<small v-if="row.custom_price" class="custom-tag">{{ t('yours') }}</small></span><span v-else class="muted">{{ t('No price') }}</span>
                    <button type="button" class="text-button" @click="editPrice(row.model, row.price)">{{ t(row.price ? 'Edit' : 'Set price') }}</button>
                    <button v-if="row.custom_price" type="button" class="text-button" :disabled="savingPrice" @click="savePrice(row.model, true)">{{ t('Reset') }}</button>
                  </template>
                </td>
              </tr>
            </tbody>
          </table></div>
        </section>

        <section class="usage-card">
          <header class="usage-card-head"><h2>{{ t('Top sessions') }}</h2><small>{{ t('{shown} of {count}', { shown: Math.min(report.conversations.length, 30), count: report.conversation_count }) }}</small></header>
          <div class="table-wrap"><table class="usage-table sessions">
            <thead><tr><th>{{ t('Session') }}</th><th>{{ t('Models') }}</th><th>{{ t('Tokens') }}</th><th>{{ t('Cost') }}</th><th>{{ t('Last active') }}</th></tr></thead>
            <tbody>
              <tr v-for="row in report.conversations.slice(0, 30)" :key="row.provider + row.conversation" :class="{ link: !!row.session_id && sessions.some(item => item.id === row.session_id) }" @click="openConversation(row.session_id)">
                <td><span class="model-cell"><ProviderIcon :provider="row.provider" :size="14" /><span><strong>{{ row.title ?? (workspaceFor(row.cwd)?.name ?? row.cwd.split('/').filter(Boolean).pop() ?? providerLabel(row.provider)) }}</strong><small>{{ row.title ? row.cwd : row.conversation.slice(0, 8) }}</small></span></span></td>
                <td class="muted">{{ row.models.join(', ') }}</td>
                <td>{{ formatTokens(row.total) }}</td>
                <td>{{ row.cost ? formatCost(row.cost) : '—' }}</td>
                <td class="muted">{{ when(row.last_at) }}</td>
              </tr>
            </tbody>
          </table></div>
        </section>
      </template>
    </div>
  </PageShell>
</template>

<style scoped>
.usage-page{display:flex;flex-direction:column;gap:var(--space-3)}
.usage-range{display:inline-flex;padding:3px;border-radius:var(--radius-md);background:var(--fill)}
.usage-range button{min-height:28px;padding:0 10px;border:0;border-radius:var(--radius-sm);background:none;color:var(--ink-soft);font-size:var(--text-sm);cursor:pointer}
.usage-range button.selected{background:var(--surface);color:var(--ink);font-weight:550;box-shadow:var(--shadow-sm)}
.usage-tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:var(--space-3)}
.usage-tile{display:flex;flex-direction:column;gap:4px;padding:var(--space-4);border-radius:var(--radius-lg);background:var(--surface);box-shadow:var(--shadow-card)}
.usage-tile>span{font-size:var(--text-sm);color:var(--ink-soft);font-weight:550}
.usage-tile strong{font-size:var(--text-3xl);font-weight:650;letter-spacing:-.5px;color:var(--ink);font-variant-numeric:tabular-nums;line-height:1.15}
.usage-tile strong em{font-style:normal;font-size:var(--text-lg);color:var(--muted);margin-left:2px}
.usage-tile small{font-size:var(--text-xs);color:var(--muted)}
.usage-tile small.up{color:var(--warn-ink)}
.usage-card{display:flex;flex-direction:column;gap:var(--space-3);padding:var(--space-4);border-radius:var(--radius-lg);background:var(--surface);box-shadow:var(--shadow-card);min-width:0}
.usage-card h2{display:flex;align-items:center;gap:6px;font-size:var(--text-md);font-weight:600;color:var(--ink)}
.usage-card-head{display:flex;align-items:baseline;justify-content:space-between;gap:var(--space-3);flex-wrap:wrap}
.usage-card-head small,.usage-footnote{font-size:var(--text-xs);color:var(--muted)}
.usage-insights{background:var(--accent-soft);box-shadow:none}
.usage-insights h2{color:var(--accent-ink)}
.usage-insights ul{margin:0;padding-left:18px;display:flex;flex-direction:column;gap:4px;font-size:var(--text-md);color:var(--ink);line-height:1.6}
.usage-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(360px,1fr));gap:var(--space-3);align-items:start}
.composition{display:flex;gap:2px;height:14px;border-radius:var(--radius-full);overflow:hidden;background:var(--fill)}
.composition span{min-width:2px;flex-basis:0}
.legend{list-style:none;margin:0;padding:0;display:flex;flex-wrap:wrap;gap:6px var(--space-4);font-size:var(--text-sm);color:var(--ink-soft)}
.legend li{display:inline-flex;align-items:center;gap:6px}
.legend i{width:10px;height:10px;border-radius:var(--radius-xs);flex:none}
.legend strong{color:var(--ink);font-weight:600;font-variant-numeric:tabular-nums}
.legend small{color:var(--muted);font-variant-numeric:tabular-nums}
.day-chart{position:relative}
.day-chart svg{display:block;width:100%;height:auto;overflow:visible}
.grid line{stroke:var(--border);stroke-width:1;vector-effect:non-scaling-stroke}
.grid text,.axis{fill:var(--muted);font-family:var(--font-body)}
.hit{fill:transparent}
.day-chart rect.dim{opacity:.35}
.day-chart rect{transition:opacity var(--duration-fast)}
.chart-tip{position:absolute;top:0;transform:translateX(-50%);z-index:2;display:flex;flex-direction:column;gap:3px;min-width:170px;padding:8px 10px;border-radius:var(--radius-md);background:var(--surface);box-shadow:var(--shadow-lg),0 0 0 1px var(--border);font-size:var(--text-xs);color:var(--ink-soft);pointer-events:none}
.chart-tip strong{color:var(--ink);font-size:var(--text-sm)}
.chart-tip span{display:flex;align-items:center;gap:6px}
.chart-tip i{width:8px;height:8px;border-radius:var(--radius-xs)}
.chart-tip b{margin-left:auto;color:var(--ink);font-variant-numeric:tabular-nums}
.chart-tip .tip-total{border-top:1px solid var(--border);padding-top:3px}
.heatmap{display:grid;grid-template-columns:34px repeat(24,minmax(0,1fr));gap:3px;align-items:center}
.heat-label,.heat-hour{font-size:var(--text-xs);color:var(--muted)}
.heat-cell{aspect-ratio:1;border-radius:var(--radius-xs);min-height:10px}
.share-list{list-style:none;margin:0;padding:0;display:flex;flex-direction:column;gap:10px}
.share-list li{display:grid;grid-template-columns:minmax(0,1.3fr) minmax(0,1fr) 56px;align-items:center;gap:var(--space-3);font-size:var(--text-sm)}
.share-name{display:flex;flex-direction:column;min-width:0}
.share-name strong{font-weight:550;color:var(--ink);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.share-name small{font-size:var(--text-xs);color:var(--muted);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.share-bar{display:block;height:6px;border-radius:var(--radius-full);background:var(--fill);overflow:hidden}
.share-bar i{display:block;height:100%;border-radius:inherit;background:var(--accent)}
.share-bar.inline{margin-top:6px;max-width:260px}
.share-list b{text-align:right;font-weight:600;color:var(--ink);font-variant-numeric:tabular-nums}
.table-wrap{overflow-x:auto}
.usage-table{width:100%;border-collapse:collapse;font-size:var(--text-sm)}
.usage-table th{text-align:left;font-weight:500;font-size:var(--text-xs);color:var(--muted);padding:0 var(--space-2) 6px;border-bottom:1px solid var(--border);white-space:nowrap}
.usage-table td{padding:8px var(--space-2);border-bottom:1px solid var(--border);color:var(--ink-soft);font-variant-numeric:tabular-nums;vertical-align:middle}
.usage-table tr:last-child td{border-bottom:0}
.usage-table td:first-child{color:var(--ink)}
.usage-table tr.link{cursor:pointer}
.usage-table tr.link:hover td{background:var(--sunken)}
.model-cell{display:flex;align-items:center;gap:8px;min-width:0}
.model-cell>span{display:flex;flex-direction:column;min-width:0}
.model-cell small{font-size:var(--text-xs);color:var(--muted)}
.muted{color:var(--muted)}
.price-cell{white-space:nowrap}
.price-cell input{width:68px;height:28px;margin-right:4px;padding:0 6px;border:1px solid var(--line);border-radius:var(--radius-sm);background:var(--surface);color:var(--ink);font-size:var(--text-sm)}
.price-cell .text-button{margin-left:8px}
.custom-tag{margin-left:6px;padding:0 6px;border-radius:var(--radius-full);background:var(--accent-soft);color:var(--accent-ink);font-size:var(--text-xs)}
.allowance-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(260px,1fr));gap:var(--space-3)}
.allowance{display:flex;flex-direction:column;gap:10px;padding:var(--space-3);border-radius:var(--radius-md);background:var(--sunken)}
.allowance header{display:flex;align-items:center;gap:8px;font-size:var(--text-sm)}
.allowance header strong{color:var(--ink);font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.allowance header span{margin-left:auto;color:var(--muted);font-size:var(--text-xs);white-space:nowrap}
.allowance-bar{height:8px;border-radius:var(--radius-full);background:var(--fill);overflow:hidden}
.allowance-bar i{display:block;width:var(--used);height:100%;border-radius:inherit;background:var(--accent)}
.allowance dl{margin:0;display:flex;flex-direction:column;gap:4px;font-size:var(--text-sm)}
.allowance dl div{display:flex;justify-content:space-between;gap:var(--space-3)}
.allowance dt{color:var(--muted)}
.allowance dd{margin:0;color:var(--ink);font-variant-numeric:tabular-nums;text-align:right}
.allowance-note{font-size:var(--text-xs);color:var(--warn-ink)}
@media (max-width:760px){.usage-grid{grid-template-columns:minmax(0,1fr)}.usage-tiles{grid-template-columns:repeat(2,minmax(0,1fr))}.usage-tile strong{font-size:var(--text-2xl)}.share-list li{grid-template-columns:minmax(0,1fr) 64px 50px}}
</style>
