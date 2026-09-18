/**
 * Keeping a chip's panel on screen.
 *
 * The panel hangs from the chip's left edge, which is where it belongs and
 * where it stays on a wide pane. On a phone the rightmost chip is close enough
 * to the edge that its panel would hang off it, so it slides left — by exactly
 * as much as it overhangs, and never past the left margin, because a panel that
 * overshoots is no easier to read than one that is cut off.
 */
export function chipPanelOffset(anchorLeft: number, panelWidth: number, viewportWidth: number, inset = 8): number {
  const overhang = anchorLeft + panelWidth - (viewportWidth - inset);
  if (overhang <= 0) return 0;
  const shift = Math.min(overhang, Math.max(0, anchorLeft - inset));
  return shift ? -shift : 0;
}
