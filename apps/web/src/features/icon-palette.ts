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

const ICON_TONES: Record<string, keyof typeof ICON_PALETTE> = {
  folder: "gold", archive: "gold", clock: "gold",
  git: "orange",
  spark: "purple", account: "purple", settings: "purple",
  file: "blue", terminal: "blue", image: "blue", play: "blue", download: "blue", info: "blue",
  close: "slate", stop: "rose",
  // Panel toggles are chrome, not content: keep them neutral so a collapsed
  // panel does not read as an active state.
  panelLeft: "slate", panelRight: "slate",
};

export function iconColor(name: string): string {
  return ICON_PALETTE[Object.hasOwn(ICON_TONES, name) ? ICON_TONES[name] : "teal"];
}

export function providerColor(provider: string): string {
  return provider === "claude_code" ? ICON_PALETTE.orange : provider === "codex" ? ICON_PALETTE.purple : ICON_PALETTE.blue;
}
