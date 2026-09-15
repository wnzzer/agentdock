<script setup lang="ts">
import { ref } from "vue";
import { errorMessage, json, request } from "./api";
import ModalDialog from "./ModalDialog.vue";
import { useI18n } from "../i18n";
const { t } = useI18n();
const emit = defineEmits<{ authenticated: [] }>();
const token = ref(""), error = ref(""), busy = ref(false);
async function signIn() {
  if (!token.value || busy.value) return;
  busy.value = true; error.value = "";
  try { await request("/auth", json("POST", { token: token.value })); token.value = ""; emit("authenticated"); }
  catch (cause) { error.value = errorMessage(cause); }
  finally { busy.value = false; }
}
</script>
<template><ModalDialog :title="t('Connect to AgentDock')" :closable="false"><form class="form-stack" @submit.prevent="signIn"><p class="form-description">{{ t('This host requires its AgentDock access token. The browser receives a protected session cookie; the token is not stored in browser storage.') }}</p><label>{{ t('Host access token') }}<input v-model="token" type="password" required autocomplete="current-password" autofocus /></label><div v-if="error" class="inline-error" role="alert">{{ error }}</div><button class="primary-button" :disabled="busy || !token">{{ t(busy ? 'Connecting…' : 'Connect securely') }}</button></form></ModalDialog></template>
