/**
 * Key sequences for the on-screen controls a touch device needs.
 *
 * A phone has no arrow keys and no Enter, which is exactly what a full-screen
 * client command such as `/config` asks for. These produce the bytes a physical
 * key would, so the client cannot tell the difference.
 */
export type ArrowKey = "up" | "down";

/**
 * Arrow keys have two encodings, and which one is correct is the terminal's
 * state rather than a preference. A full-screen program usually switches the
 * terminal into application cursor mode (DECCKM), after which it expects `ESC O
 * A`; outside that mode the same key is `ESC [ A`. Sending the wrong one is
 * silently ignored or, worse, printed, so the current mode decides.
 */
export function arrowSequence(key: ArrowKey, applicationCursorKeys: boolean): string {
  const final = key === "up" ? "A" : "B";
  return applicationCursorKeys ? `O${final}` : `[${final}`;
}

/** What the Enter key sends: a carriage return, not a newline. */
export const ENTER_SEQUENCE = "\r";

/**
 * How many arrow presses move a list selection from one row to another.
 *
 * This is the tap-to-select gesture, and it rests on an assumption worth
 * stating: that the program keeps the cursor on the row it has highlighted.
 * List menus generally do, because that is how a screen reader follows them.
 * A program that parks its cursor elsewhere will move by the wrong amount, so
 * the gesture is only offered where a full-screen program is actually drawing.
 *
 * The count is bounded because a mistaken row — or a tap in a program that
 * does not behave this way — should cost a few keystrokes, not hundreds.
 */
export function selectionPresses(cursorRow: number, targetRow: number, limit = 40): { key: ArrowKey; count: number } | undefined {
  if (!Number.isInteger(cursorRow) || !Number.isInteger(targetRow)) return undefined;
  const distance = targetRow - cursorRow;
  if (distance === 0) return undefined;
  const count = Math.min(Math.abs(distance), limit);
  return { key: distance < 0 ? "up" : "down", count };
}

/** The bytes for `count` presses of one arrow key. */
export function repeatArrow(key: ArrowKey, count: number, applicationCursorKeys: boolean): string {
  return arrowSequence(key, applicationCursorKeys).repeat(Math.max(0, count));
}

/**
 * Whether a pointer gesture was a tap rather than a drag or a text selection.
 *
 * Selecting text and scrolling both begin as a touch on the same surface, so a
 * gesture only counts as a tap when the finger stayed put, lifted quickly, and
 * left nothing selected behind it.
 */
export function isTap(gesture: { movedPx: number; elapsedMs: number; hasSelection: boolean }): boolean {
  return !gesture.hasSelection && gesture.movedPx <= 10 && gesture.elapsedMs <= 500;
}
