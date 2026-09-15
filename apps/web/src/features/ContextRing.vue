<script setup lang="ts">
import { computed } from 'vue';
import { compactTokens, contextMeter, type ContextUsage } from './context-meter';
import { useI18n } from '../i18n';

/** A ring showing how much of the model's context window is in use. */
const props = withDefaults(defineProps<{ usage?: ContextUsage; size?: number }>(), { size: 22 });
const { t } = useI18n();
const meter = computed(() => contextMeter(props.usage));
const geometry = computed(() => {
  const stroke = Math.max(2, Math.round(props.size / 9));
  const radius = (props.size - stroke) / 2;
  const circumference = 2 * Math.PI * radius;
  return { stroke, radius, circumference, center: props.size / 2 };
});
const label = computed(() => meter.value
  ? t('Context {percent}% · {used} of {window} tokens', { percent: meter.value.percent, used: compactTokens(meter.value.used), window: compactTokens(meter.value.window) })
  : '');
</script>

<template>
  <span v-if="meter" :class="['context-ring', meter.level]" :title="label">
    <svg :width="size" :height="size" :viewBox="`0 0 ${size} ${size}`" role="img" :aria-label="label">
      <circle class="context-ring-track" :cx="geometry.center" :cy="geometry.center" :r="geometry.radius" fill="none" :stroke-width="geometry.stroke"/>
      <circle
        class="context-ring-value"
        :cx="geometry.center" :cy="geometry.center" :r="geometry.radius" fill="none"
        :stroke-width="geometry.stroke" stroke-linecap="round"
        :stroke-dasharray="geometry.circumference"
        :stroke-dashoffset="geometry.circumference * (1 - meter.fraction)"
        :transform="`rotate(-90 ${geometry.center} ${geometry.center})`"
      />
    </svg>
    <span class="context-ring-text">{{ meter.percent }}%</span>
    <!-- The share alone reads as arbitrary when the window is unusual: 143k of a
         1M model is 14%, which looks wrong until the denominator is visible. -->
    <span class="context-ring-scale">{{ compactTokens(meter.used) }}/{{ compactTokens(meter.window) }}</span>
  </span>
</template>

<style scoped>
.context-ring{display:inline-flex;align-items:center;gap:5px;color:#647681;font-size:10px;line-height:1;white-space:nowrap}
.context-ring svg{display:block;flex-shrink:0}
.context-ring-scale{opacity:.72;font-variant-numeric:tabular-nums}
.context-ring-track{stroke:#e3eaec}
.context-ring-value{stroke:var(--teal);transition:stroke-dashoffset .3s ease}
.context-ring.high .context-ring-value{stroke:#C77916}
.context-ring.high .context-ring-text{color:#9c6a1c}
.context-ring.full .context-ring-value{stroke:#D94B55}
.context-ring.full .context-ring-text{color:#b2404c}
.context-ring-text{font-variant-numeric:tabular-nums}
@media (prefers-reduced-motion: reduce){.context-ring-value{transition:none}}
</style>
