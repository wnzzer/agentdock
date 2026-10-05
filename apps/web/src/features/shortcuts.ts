/**
 * Keyboard shortcuts for the whole app, in one table, so the help sheet and
 * the handler can never disagree about what a key does.
 *
 * "mod" is ⌘ on a Mac and Ctrl elsewhere. A focused terminal keeps every key:
 * Ctrl+K, Ctrl+B and the rest mean something to the shell or the CLI inside it,
 * and taking them away would break the very thing the terminal is for. Typing
 * in a field keeps plain keys; only chords with mod reach the app there.
 */
export interface Chord { key: string; mod?: boolean; shift?: boolean; alt?: boolean }
export interface Shortcut { id: string; chord: Chord; label: string; group: "General" | "Navigation" | "Sessions" }

export const SHORTCUTS: readonly Shortcut[] = [
  { id: "palette", chord: { key: "k", mod: true }, label: "Search and commands", group: "General" },
  { id: "help", chord: { key: "/", mod: true }, label: "Keyboard shortcuts", group: "General" },
  { id: "settings", chord: { key: ",", mod: true }, label: "Settings", group: "General" },
  { id: "sidebar", chord: { key: "b", mod: true }, label: "Show or hide the sidebar", group: "Navigation" },
  { id: "files", chord: { key: "e", mod: true, shift: true }, label: "Show or hide the file panel", group: "Navigation" },
  { id: "sessions", chord: { key: "s", mod: true, shift: true }, label: "All sessions", group: "Navigation" },
  { id: "system", chord: { key: "m", mod: true, shift: true }, label: "System", group: "Navigation" },
  { id: "canvas", chord: { key: "h", mod: true, shift: true }, label: "Back to the workspace canvas", group: "Navigation" },
  { id: "new-session", chord: { key: "o", mod: true, shift: true }, label: "New session…", group: "Sessions" },
];

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** ⌘⇧O on a Mac, Ctrl+Shift+O elsewhere. */
export function formatChord(chord: Chord, mac = isMac): string {
  const key = chord.key.length === 1 ? chord.key.toUpperCase() : chord.key;
  const parts = [chord.mod ? (mac ? "⌘" : "Ctrl") : "", chord.alt ? (mac ? "⌥" : "Alt") : "", chord.shift ? (mac ? "⇧" : "Shift") : "", key].filter(Boolean);
  return mac ? parts.join("") : parts.join("+");
}

export function matches(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">, chord: Chord, mac = isMac): boolean {
  const mod = mac ? event.metaKey : event.ctrlKey;
  if (Boolean(chord.mod) !== mod || Boolean(chord.shift) !== event.shiftKey || Boolean(chord.alt) !== event.altKey) return false;
  // Shift changes the character ("/" becomes "?"), so compare case-insensitively and accept both.
  return event.key.toLowerCase() === chord.key.toLowerCase() || (chord.key === "/" && event.key === "?");
}

/** Where a key must be left alone: inside a terminal, always; in a field, unless it is a chord. */
export function keyBelongsToTarget(target: EventTarget | null, chord: Chord): boolean {
  const element = target instanceof Element ? target : null;
  if (element?.closest(".xterm, .terminal-host")) return true;
  const field = element?.closest("input, textarea, select, [contenteditable='true']");
  return Boolean(field) && !chord.mod;
}

export function shortcutFor(event: KeyboardEvent, list: readonly Shortcut[] = SHORTCUTS): Shortcut | undefined {
  if (event.isComposing || event.key === "Process" || event.key === "Dead") return undefined;
  const hit = list.find(item => matches(event, item.chord));
  return hit && !keyBelongsToTarget(event.target, hit.chord) ? hit : undefined;
}
