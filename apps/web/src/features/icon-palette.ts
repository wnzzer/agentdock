/** Shared foreground tokens for the light UI. Keep provider identity consistent
 * in navigation, tabs and menus; solid buttons can opt into inherited color. */
export const ICON_PALETTE = {
  teal: "#0C8376",
  gold: "#A77C2D",
  orange: "#B75B27",
  purple: "#7552B8",
  blue: "#326EB4",
  rose: "#B54E65",
  slate: "#708797",
} as const;

/**
 * Functional icons carry no colour of their own: they take the colour of the
 * text beside them, so a row reads in one tone and a selected one in teal.
 * A palette per icon (gold folders, orange git, purple settings, blue files)
 * made the chrome look busier than anything on it. Only stopping keeps a hue,
 * because it destroys something; providers keep theirs, being identity.
 */
const ICON_TONES: Record<string, keyof typeof ICON_PALETTE> = { stop: "rose" };

export function iconColor(name: string): string {
  return Object.hasOwn(ICON_TONES, name) ? ICON_PALETTE[ICON_TONES[name]] : "currentColor";
}

export function providerColor(provider: string): string {
  return provider === "claude_code" ? ICON_PALETTE.orange : provider === "codex" ? ICON_PALETTE.purple : ICON_PALETTE.blue;
}
