/**
 * Composer suggestions for the slash commands a native client advertised.
 *
 * The list only ever comes from the client's own startup announcement, so the
 * composer offers exactly what that client accepts. A client that advertises
 * nothing gets no menu rather than a plausible-looking list of commands it would
 * reject.
 */
export interface SlashQuery {
  /** The typed command without its leading slash. */
  term: string;
  /** Characters of the composer the completion replaces. */
  length: number;
}

/**
 * A slash command is only being typed when the slash opens the message and no
 * whitespace has followed it yet. `/model` is a command; `/model sonnet` has
 * moved on to its argument, and `see /etc/hosts` was never one.
 */
export function slashQuery(text: string, caret: number): SlashQuery | undefined {
  if (!text.startsWith('/') || caret < 1) return undefined;
  const upToCaret = text.slice(0, caret);
  if (upToCaret.length > 80) return undefined;
  const term = upToCaret.slice(1);
  if (/[\s]/.test(term)) return undefined;
  // Only a prefix of the first token is being completed.
  const token = /^[^\s]*/.exec(text.slice(1))?.[0] ?? '';
  if (!token.startsWith(term)) return undefined;
  return { term, length: caret };
}

/**
 * Rank by how the command is being typed: an exact prefix first, then a match
 * anywhere in the name. Ordering within a group keeps the client's own order,
 * which puts its built-ins ahead of user-defined ones.
 */
export function matchCommands(commands: string[], term: string, limit = 8): string[] {
  const needle = term.toLowerCase();
  if (!needle) return commands.slice(0, limit);
  const prefix: string[] = [], contains: string[] = [];
  for (const name of commands) {
    const lower = name.toLowerCase();
    if (lower.startsWith(needle)) prefix.push(name);
    else if (lower.includes(needle)) contains.push(name);
  }
  return [...prefix, ...contains].slice(0, limit);
}

/** Replace the typed prefix with the chosen command and a trailing space. */
export function applyCommand(text: string, query: SlashQuery, command: string): { text: string; caret: number } {
  const completed = `/${command} `;
  return { text: completed + text.slice(query.length), caret: completed.length };
}

/** Move through the list, wrapping at both ends. */
export function moveHighlight(index: number, count: number, delta: number): number {
  if (count <= 0) return 0;
  return (index + delta + count) % count;
}

/**
 * Whether a message would be sent as a command the client cannot act on.
 *
 * Codex's app-server protocol has no slash-command concept at all, so `/model`
 * typed here would reach the model as literal prose and quietly do nothing the
 * user intended. Saying so is better than sending it; the native terminal view
 * still runs that client's own commands.
 *
 * `announced` is what separates "this client has none" from "it has not told us
 * yet". A session that has never reached ready — a window created a moment ago —
 * has an empty list for the second reason, and refusing its `/model` there would
 * block a command the client does in fact run. Only a client that has announced
 * its list and left it empty is known to have none.
 */
export function unsupportedCommand(text: string, commands: string[], announced = true): string | undefined {
  if (!announced || commands.length) return undefined;
  const match = /^\/([A-Za-z0-9][\w:-]{0,63})\s*$/.exec(text.trim());
  return match?.[1];
}
