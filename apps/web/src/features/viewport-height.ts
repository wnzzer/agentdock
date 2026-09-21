/**
 * How tall the page actually is, once a phone keyboard is in the way.
 *
 * `100dvh` accounts for a browser's own chrome and nothing else: an on-screen
 * keyboard covers the bottom of the page without the page ever being told, so
 * whatever lives down there — a terminal's prompt, and the command list a
 * client draws under it — is behind the keyboard rather than above it.
 *
 * The visual viewport does know, so it is what the shell is sized from while a
 * keyboard is open, and `dvh` is left alone the rest of the time.
 */
export interface Visual { height: number; scale: number }

/** A keyboard, as distinct from the several other things that move this number. */
const KEYBOARD = 80;

export function shellHeight(visual: Visual | undefined, innerHeight: number): number | undefined {
  // Pinching zooms the visual viewport without hiding anything. Resizing the
  // page to a pinch would fight the gesture rather than help it.
  if (!visual || visual.scale > 1.01) return undefined;
  const hidden = innerHeight - visual.height;
  // A collapsing URL bar moves this by tens of pixels and is already `dvh`'s
  // job. Only a keyboard takes this much, and only then is this worth doing.
  return hidden >= KEYBOARD ? Math.round(visual.height) : undefined;
}
