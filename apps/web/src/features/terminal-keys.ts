/**
 * Key sequences for the on-screen controls a touch device needs.
 *
 * A phone has no arrow keys and no Enter, which is exactly what a full-screen
 * client command such as `/config` asks for. These produce the bytes a physical
 * key would, so the client cannot tell the difference.
 */
export type ArrowKey = "up" | "down" | "left" | "right";

/**
 * Arrow keys have two encodings, and which one is correct is the terminal's
 * state rather than a preference. A full-screen program usually switches the
 * terminal into application cursor mode (DECCKM), after which it expects `ESC O
 * A`; outside that mode the same key is `ESC [ A`. Sending the wrong one is
 * silently ignored or, worse, printed, so the current mode decides.
 */
export function arrowSequence(key: ArrowKey, applicationCursorKeys: boolean): string {
  const final = { up: "A", down: "B", right: "C", left: "D" }[key];
  return applicationCursorKeys ? `O${final}` : `[${final}`;
}

/** What the Enter key sends: a carriage return, not a newline. */
export const ENTER_SEQUENCE = "\r";

/**
 * Escape, which a phone keyboard does not have at all and which is how a
 * running agent is interrupted or a menu dismissed.
 */
export const ESCAPE_SEQUENCE = "\x1b";

/** Tab, and Shift+Tab (CBT), which agent clients use to cycle their modes. */
export const TAB_SEQUENCE = "\t";
export const BACKTAB_SEQUENCE = "\x1b[Z";

/**
 * What a key typed while Ctrl is held sends.
 *
 * A phone keyboard has no Ctrl, so the bar offers a one-shot modifier and the
 * next character typed on the soft keyboard is turned into its control code:
 * the letter's low five bits, which is what a terminal has always sent. Only a
 * single character has a control form; anything else -- a word from an input
 * method, a paste -- passes through unchanged rather than being mangled.
 */
export function ctrlSequence(data: string): string {
  if (data.length !== 1) return data;
  if (data === " " || data === "@" || data === "2") return "\x00";
  if (data === "?") return "\x7f";
  const code = data.toUpperCase().charCodeAt(0);
  // `A`..`Z` and the five punctuation keys after them: [ \ ] ^ _
  if (code >= 0x41 && code <= 0x5f) return String.fromCharCode(code & 0x1f);
  return data;
}

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
