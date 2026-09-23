/**
 * Carrying a conversation over to a different agent.
 *
 * Claude Code and Codex keep their histories in formats neither can read, so a
 * conversation cannot be moved between them as it is. What can be moved is what
 * was said: this writes the visible conversation out as text for the other
 * client's first message. It is a transcript, not a resumed session -- the new
 * client sees the words, not the tool results or files the old one had open --
 * and the text is placed in the composer rather than sent, so it is read and
 * finished by the user first.
 */

/** A message the transcript is made from. Tool cards and approvals are left
 * out: their output is often large, and the new agent can look again itself. */
export interface HandoffMessage { role: 'user' | 'assistant'; text: string }

/** The server refuses a message over 32768 bytes; the rest of this budget is
 * left for the framing and for whatever the user adds after it. */
export const HANDOFF_TRANSCRIPT_BYTES = 26 * 1024;

const encoder = new TextEncoder();
const bytes = (text: string) => encoder.encode(text).byteLength;

/** The end of `text` that fits in `budget` bytes, cut on a character boundary. */
function tail(text: string, budget: number): string {
  if (bytes(text) <= budget) return text;
  const chars = [...text];
  let used = 0, start = chars.length;
  while (start > 0) {
    const size = bytes(chars[start - 1]);
    if (used + size > budget) break;
    used += size; start--;
  }
  return chars.slice(start).join('');
}

export interface HandoffLabels {
  /** Name for the user's turns, e.g. "You". */
  user: string;
  /** Name for the old agent's turns, e.g. "Claude Code". */
  assistant: string;
  /** Said once, in place of whatever did not fit. */
  omitted: string;
}

/**
 * The most recent messages that fit in `budget`, oldest first.
 *
 * Recent turns are the ones the next agent needs, so it is the start that is
 * dropped. A single message larger than everything allowed keeps its end.
 */
export function handoffTranscript(messages: readonly HandoffMessage[], labels: HandoffLabels, budget = HANDOFF_TRANSCRIPT_BYTES): { text: string; omitted: boolean } {
  const blocks = messages
    .filter(message => message.text.trim())
    .map(message => `${message.role === 'user' ? labels.user : labels.assistant}:\n${message.text.trim()}`);
  const kept: string[] = [];
  let used = 0;
  for (let index = blocks.length - 1; index >= 0; index--) {
    const size = bytes(blocks[index]) + 2;
    if (used + size > budget) {
      if (!kept.length) kept.unshift(tail(blocks[index], budget));
      break;
    }
    kept.unshift(blocks[index]); used += size;
  }
  const omitted = kept.length < blocks.length || (kept.length === 1 && kept[0] !== blocks[blocks.length - 1]);
  return { text: (omitted ? `(${labels.omitted})\n\n` : '') + kept.join('\n\n'), omitted };
}

/** Sessions just opened by a handoff, shared across panes: the new session
 * mounts in a different pane from the one that made it. That pane puts the
 * caret after the written-out conversation, where the next instruction goes. */
export const handoffArrivals = new Set<string>();
