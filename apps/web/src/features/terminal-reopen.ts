import type { Session } from "@agentdock/protocol";
import { json, request } from "./api";

/**
 * Open a structured session's conversation in a real terminal client.
 *
 * Structured mode drives the same client over a JSON pipe, which is exactly what
 * makes an interactive command such as `/config` unusable: the pipe carries one
 * turn at a time, and a full-screen TUI has nowhere to run. Rather than trying
 * to imitate that TUI over the pipe, this asks the server for a genuine terminal
 * session on the same conversation — same client, same account, same
 * configuration home, same native session id.
 *
 * It returns a *new* session rather than converting the current one, so the
 * conversation pane is left untouched and its history survives closing the
 * terminal.
 */
export const terminalReopen = (session: Pick<Session, "id">) =>
  request<Session>(`/sessions/${encodeURIComponent(session.id)}/terminal`, json("POST"));

/**
 * Whether the escape hatch is worth offering.
 *
 * A conversation only exists once the client has reported a session id, so
 * before the first turn there is nothing to resume and the server would refuse.
 * Offering the action anyway would be a button whose only outcome is an error.
 */
export function canReopenInTerminal(session: Pick<Session, "provider" | "interaction_mode" | "provider_session_id">, capabilities: { sessionTerminalEscape?: boolean } = {}): boolean {
  if (capabilities.sessionTerminalEscape !== true) return false;
  if (session.provider === "terminal") return false;
  if (session.interaction_mode !== "structured") return false;
  return !!session.provider_session_id;
}
