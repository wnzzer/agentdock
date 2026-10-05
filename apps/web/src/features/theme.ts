import { ref } from "vue";

/**
 * Light, dark, or whatever the system says. Like the language, it belongs to
 * this browser rather than the server: a phone and a laptop may differ.
 *
 * "system" leaves <html> without a data-theme, so the stylesheet's
 * prefers-color-scheme block decides; the other two pin it.
 */
export type ThemePreference = "system" | "light" | "dark";
const KEY = "agentdock.theme";

function stored(): ThemePreference {
  try {
    const value = localStorage.getItem(KEY);
    return value === "light" || value === "dark" ? value : "system";
  } catch { return "system"; }
}

const query = typeof window !== "undefined" && window.matchMedia ? window.matchMedia("(prefers-color-scheme: dark)") : undefined;
export const themePreference = ref<ThemePreference>(stored());
/** Whether the interface is dark right now, for what CSS cannot reach (the terminal's canvas). */
export const darkScheme = ref(false);

function apply() {
  const preference = themePreference.value;
  if (typeof document !== "undefined") {
    if (preference === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = preference;
  }
  darkScheme.value = preference === "dark" || (preference === "system" && query?.matches === true);
}
query?.addEventListener("change", apply);
apply();

export function setThemePreference(preference: ThemePreference) {
  themePreference.value = preference;
  try {
    if (preference === "system") localStorage.removeItem(KEY);
    else localStorage.setItem(KEY, preference);
  } catch { /* Private browsing: the choice lasts for this page. */ }
  apply();
}
