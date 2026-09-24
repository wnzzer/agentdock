import { ref } from "vue";
import type { ProviderKind } from "@agentdock/protocol";
import { json, request } from "./api";

/**
 * What a new session starts with, per provider. Kept by the server, so every
 * device opens sessions the same way; each value is only a default the
 * new-session dialog and the session's own chips can still change.
 */
export interface ProviderPreference { endpoint_profile_id?: string | null; effort?: string | null; permission?: string | null }
export interface Preferences { claude_code: ProviderPreference; codex: ProviderPreference }
export type PreferenceProvider = keyof Preferences;

export const PERMISSION_CHOICES: Record<PreferenceProvider, readonly string[]> = {
  claude_code: ["ask", "plan", "accept_edits", "danger"],
  codex: ["ask", "danger"],
};

/** The one copy the page reads; loaded once and replaced on save. */
export const preferences = ref<Preferences>({ claude_code: {}, codex: {} });
let loading: Promise<Preferences> | undefined;

export function loadPreferences(force = false): Promise<Preferences> {
  if (!loading || force) {
    loading = request<Preferences>("/preferences")
      .then(value => (preferences.value = value))
      // An older server has no preferences; that is the same as none set.
      .catch(() => preferences.value);
  }
  return loading;
}

export async function savePreferences(next: Preferences): Promise<Preferences> {
  const saved = await request<Preferences>("/preferences", json("PUT", next));
  preferences.value = saved; loading = Promise.resolve(saved);
  return saved;
}

export function preferenceFor(provider: ProviderKind, source: Preferences = preferences.value): ProviderPreference | undefined {
  return provider === "claude_code" || provider === "codex" ? source[provider] : undefined;
}
