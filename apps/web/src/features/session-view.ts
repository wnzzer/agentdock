import type { Session } from "@agentdock/protocol";

/** A view preference is local UI state, never persisted into the workspace layout. */
export type SessionView = "chat" | "native";
const preferences = new Map<string, SessionView>();

export function defaultSessionView(session: Pick<Session, "provider">): SessionView {
  return session.provider === "terminal" ? "native" : "chat";
}

export function sessionView(session: Pick<Session, "id" | "provider" | "interaction_mode">): SessionView {
  if (session.provider === "terminal") return "native";
  // Structured mode is owned by the conversation surface. A stale native
  // preference must not disguise it as a PTY.
  if (session.interaction_mode === "structured") return "chat";
  return preferences.get(session.id) ?? defaultSessionView(session);
}

export function setSessionView(session: Pick<Session, "id" | "provider">, view: SessionView): SessionView {
  const next = session.provider === "terminal" ? "native" : view;
  preferences.set(session.id, next);
  return next;
}

export function clearSessionView(sessionId: string): void { preferences.delete(sessionId); }

/** Test isolation and hot-reload cleanup; intentionally not exported to app code. */
export function resetSessionViewsForTests(): void { preferences.clear(); }
