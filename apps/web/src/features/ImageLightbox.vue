<script setup lang="ts">
import { onBeforeUnmount, onMounted } from 'vue';
import { closeLightbox, lightbox } from './image-lightbox';
import { useI18n } from '../i18n';

/** Any image clicked in a message or a rendered file, at full size. */
const { t } = useI18n();
const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape' && lightbox.value) { event.preventDefault(); closeLightbox(); } };
onMounted(() => window.addEventListener('keydown', onKey));
onBeforeUnmount(() => window.removeEventListener('keydown', onKey));
</script>

<template>
  <Teleport to="body">
    <Transition name="lightbox">
      <div v-if="lightbox" class="lightbox" role="dialog" :aria-label="lightbox.alt || t('Image preview')" @click.self="closeLightbox">
        <figure><img :src="lightbox.src" :alt="lightbox.alt" /><figcaption v-if="lightbox.alt">{{ lightbox.alt }}</figcaption></figure>
        <div class="lightbox-actions"><a :href="lightbox.src" target="_blank" rel="noopener">{{ t('Open original') }}</a><button type="button" :aria-label="t('Close')" @click="closeLightbox">×</button></div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.lightbox{position:fixed;inset:0;z-index:1000;display:grid;place-items:center;padding:48px 24px;background:#10181ecc;backdrop-filter:blur(4px);cursor:zoom-out}
.lightbox figure{margin:0;max-width:100%;max-height:100%;display:flex;flex-direction:column;align-items:center;gap:10px;cursor:default}
.lightbox img{max-width:min(1600px,100%);max-height:calc(100dvh - 140px);object-fit:contain;border-radius:8px;background:repeating-conic-gradient(#f1f4f6 0 25%,#fff 0 50%) 0 0/16px 16px;box-shadow:0 20px 60px #0008}
.lightbox figcaption{color:#e8eef1;font-size:12px}
.lightbox-actions{position:absolute;top:14px;right:16px;display:flex;align-items:center;gap:10px}
.lightbox-actions a{color:#e8eef1;font-size:12px;text-decoration:none;padding:6px 10px;border:1px solid #ffffff40;border-radius:8px}
.lightbox-actions button{width:36px;height:36px;border:0;border-radius:50%;background:#ffffff26;color:#fff;font-size:22px;line-height:1;cursor:pointer}
.lightbox-enter-active,.lightbox-leave-active{transition:opacity .18s ease}
.lightbox-enter-from,.lightbox-leave-to{opacity:0}
</style>
