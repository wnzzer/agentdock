<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import type { AccountLimit } from './account-ui';
import { accountLimitPercent, accountResetDate, quotaLevel, quotaWindowTitle, untilLabel } from './account-ui';
import { useI18n } from '../i18n';

/**
 * One official quota window, drawn as a ring.
 *
 * The number that matters is how much is left and when it comes back, so the
 * ring carries the first and the line under it the second — "resets in 2h 18m"
 * rather than a timestamp to subtract in one's head. The colour turns as the
 * window runs down, so two cards side by side say at a glance which is close.
 */
const props = defineProps<{ position: 'primary' | 'secondary'; limit?: AccountLimit }>();
const { t, locale } = useI18n();
const now = ref(Date.now());
let timer: ReturnType<typeof setInterval> | undefined;
onMounted(() => { timer = setInterval(() => { now.value = Date.now(); }, 30_000); });
onBeforeUnmount(() => { if (timer) clearInterval(timer); });

const percent = computed(() => accountLimitPercent(props.limit));
const level = computed(() => quotaLevel(percent.value));
const title = computed(() => { const name = quotaWindowTitle(props.position, props.limit); return t(name.key, name.values); });
const resetsAt = computed(() => accountResetDate(props.limit?.resets_at));
const until = computed(() => resetsAt.value ? untilLabel(resetsAt.value.getTime() - now.value) : undefined);
const resetLine = computed(() => !until.value ? t('Reset time unavailable') : until.value.soon ? t('Resetting now') : t('Resets in {time}', { time: t(until.value.key, until.value.values) }));
const resetExact = computed(() => resetsAt.value ? new Intl.DateTimeFormat(locale.value, { month: 'short', day: 'numeric', weekday: 'short', hour: '2-digit', minute: '2-digit' }).format(resetsAt.value) : '');
// r = 26 on a 64 box: circumference ≈ 163.4.
const CIRCUMFERENCE = 2 * Math.PI * 26;
const dash = computed(() => percent.value === undefined ? 0 : CIRCUMFERENCE * Math.max(0.015, percent.value / 100));
</script>

<template>
  <div :class="['quota-meter', level]" role="group" :aria-label="title">
    <svg class="quota-ring" viewBox="0 0 64 64" aria-hidden="true">
      <circle class="quota-track" cx="32" cy="32" r="26" />
      <circle v-if="percent!==undefined" class="quota-value" cx="32" cy="32" r="26" :stroke-dasharray="`${dash} ${CIRCUMFERENCE}`" />
    </svg>
    <span class="quota-figure"><strong>{{ percent===undefined ? '—' : Math.round(percent) }}</strong><small v-if="percent!==undefined">%</small></span>
    <div class="quota-copy">
      <span class="quota-title">{{ title }}</span>
      <span class="quota-left">{{ percent===undefined ? t('Unavailable') : t('{count}% left', { count: Math.max(0, Math.round(100 - percent)) }) }}</span>
      <span class="quota-reset" :title="resetExact">{{ resetLine }}</span>
      <small v-if="resetExact" class="quota-exact">{{ resetExact }}</small>
    </div>
  </div>
</template>

<style scoped>
.quota-meter{--quota:var(--teal);--quota-soft:var(--teal-soft);position:relative;display:grid;grid-template-columns:64px minmax(0,1fr);align-items:center;gap:14px;padding:14px 16px;border:1px solid var(--border);border-radius:14px;background:linear-gradient(180deg,var(--surface),var(--sunken));min-width:0}
.quota-meter.warn{--quota:#c88a1c;--quota-soft:#fdf3e1}
.quota-meter.danger{--quota:#c2415a;--quota-soft:#fdecef}
.quota-meter.unknown{--quota:var(--muted)}
.quota-ring{grid-row:1;grid-column:1;width:64px;height:64px;transform:rotate(-90deg)}
.quota-track{fill:none;stroke:var(--quota-soft);stroke-width:7}
.quota-value{fill:none;stroke:var(--quota);stroke-width:7;stroke-linecap:round;transition:stroke-dasharray .6s cubic-bezier(.2,.8,.2,1)}
.quota-figure{grid-row:1;grid-column:1;display:flex;align-items:baseline;justify-content:center;color:var(--ink);pointer-events:none}
.quota-figure strong{font-size:17px;font-weight:650;letter-spacing:-.4px;font-variant-numeric:tabular-nums}
.quota-figure small{font-size:10px;font-weight:600;color:var(--ink-soft);margin-left:1px}
.quota-copy{display:flex;flex-direction:column;gap:2px;min-width:0}
.quota-title{font-size:11px;font-weight:600;letter-spacing:.3px;color:var(--ink-soft);text-transform:uppercase}
.quota-left{font-size:15px;font-weight:600;color:var(--quota);font-variant-numeric:tabular-nums}
.quota-reset{font-size:12px;color:var(--ink);margin-top:2px}
.quota-exact{font-size:10.5px;color:var(--muted);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
@media(prefers-reduced-motion:reduce){.quota-value{transition:none}}
</style>
