import type { LayoutDocument, LayoutNode, PaneNode, Session, Workspace } from "@agentdock/protocol";
import { closePane, flattenPanes, restoreCollapsed, validateLayout } from "../layout/layout-engine";
import { isEphemeralSession } from "./session-list";

export function paneString(pane: PaneNode, key: string): string | undefined {
  const value = pane.metadata?.[key];
  return typeof value === "string" && value ? value : undefined;
}
export function sessionPane(session: Session): PaneNode {
  return { type: "pane", id: `session-${session.id}`, kind: session.provider === "terminal" ? "terminal" : "agent_chat", title: session.title,
    metadata: { workspace_id: session.workspace_id, session_id: session.id, provider: session.provider } };
}
/** Update every canvas tab bound to a session without changing its pane ID. */
export function renameSessionPanes(root: LayoutNode, sessionId: string, title: string): LayoutNode {
  if (root.type === "pane") return paneString(root, "session_id") === sessionId && root.title !== title ? { ...root, title } : root;
  if (root.type === "stack") {
    let changed = false;
    const panes = root.panes.map(pane => {
      const next = renameSessionPanes(pane, sessionId, title) as PaneNode;
      changed ||= next !== pane;
      return next;
    });
    return changed ? { ...root, panes } : root;
  }
  const first = renameSessionPanes(root.first, sessionId, title);
  const second = renameSessionPanes(root.second, sessionId, title);
  return first === root.first && second === root.second ? root : { ...root, first, second };
}
/**
 * The view a session is actually bound to. A layout restored from an older
 * document can carry a pane ID that predates `session-<id>`, so locating must
 * follow the binding rather than assume the canonical ID exists.
 */
export function findSessionPaneId(root: LayoutNode, sessionId: string): string | undefined {
  if (root.type === "pane") return paneString(root, "session_id") === sessionId ? root.id : undefined;
  if (root.type === "stack") {
    for (const pane of root.panes) {
      const id = findSessionPaneId(pane, sessionId);
      if (id) return id;
    }
    return undefined;
  }
  return findSessionPaneId(root.first, sessionId) ?? findSessionPaneId(root.second, sessionId);
}
export function filePane(workspaceId: string, path: string): PaneNode {
  return { type: "pane", id: `file-${encodeURIComponent(workspaceId)}:${encodeURIComponent(path)}`,
    kind: /\.(png|jpe?g|gif|webp|svg|bmp|ico|avif|mp4|webm|mov|m4v|ogv|mp3|wav|ogg|m4a|flac|pdf)$/i.test(path) ? "file_preview" : "editor",
    title: path.split("/").pop(), metadata: { workspace_id: workspaceId, path } };
}
export function changesPane(workspaceId: string): PaneNode {
  return { type: "pane", id: `changes-${encodeURIComponent(workspaceId)}`, kind: "git_diff", title: "Changes", metadata: { workspace_id: workspaceId } };
}
export function paneSession(pane: PaneNode, sessions: Session[]): Session | undefined {
  if (pane.kind !== "agent_chat" && pane.kind !== "terminal") return undefined;
  const session = sessions.find(session => session.id === paneString(pane, "session_id"));
  if (!session || paneString(pane, "workspace_id") !== session.workspace_id) return undefined;
  if ((pane.kind === "terminal") !== (session.provider === "terminal")) return undefined;
  return session;
}
/** No fallback to whichever workspace is currently selected in the sidebar. */
export function paneWorkspace(pane: PaneNode, workspaces: Workspace[], sessions: Session[]): Workspace | undefined {
  const bound = paneString(pane, "workspace_id");
  if (!bound) return undefined;
  if (paneString(pane, "session_id") && !paneSession(pane, sessions)) return undefined;
  return workspaces.find(workspace => workspace.id === bound);
}
export function ephemeralSessionIds(sessions: readonly Session[]): string[] {
  return sessions.filter(isEphemeralSession).map(session => session.id);
}
/**
 * The persisted copy of a layout. A temporary window's record is discarded with
 * its pane, so storing the pane would resurrect a view whose session is gone.
 * The live in-memory layout keeps the pane; only what leaves this page is trimmed.
 */
export function withoutEphemeralPanes(document: LayoutDocument, sessions: readonly Session[]): LayoutDocument {
  const ids = new Set(ephemeralSessionIds(sessions));
  if (!ids.size) return document;
  const doomed = flattenPanes(document.root).filter(pane => { const id = paneString(pane, "session_id"); return !!id && ids.has(id); });
  if (!doomed.length) return document;
  let root = document.root;
  for (const pane of doomed) root = closePane(root, pane.id);
  return { ...document, root };
}
export function acceptsScopedPane(pane: PaneNode, workspaces: Workspace[], sessions: Session[]): boolean {
  if (!paneWorkspace(pane, workspaces, sessions)) return false;
  const path = pane.metadata?.path;
  return path === undefined || (typeof path === "string" && !!path && !path.startsWith("/") && !path.includes("\0") && !path.split("/").includes(".."));
}

/** One-time migration, in a new document. Old per-workspace layouts stay untouched. */
export function scopeLegacyLayout(document: LayoutDocument, ownerId: string, sessions: Session[]): LayoutDocument {
  if (!validateLayout(document)) throw new Error("Invalid saved layout");
  const restored = restoreCollapsed(document);
  const ids = new Set<string>();
  const renamed = new Map<string, string>();
  function unique(preferred: string) {
    let id = preferred, suffix = 1;
    while (ids.has(id)) id = `${preferred}-${++suffix}`;
    ids.add(id); return id;
  }
  function visit(node: LayoutNode): LayoutNode {
    if (node.type === "split") return { ...node, id: unique(node.id), first: visit(node.first), second: visit(node.second) };
    if (node.type === "stack") {
      const id = unique(node.id), panes = node.panes.map(pane => visit(pane) as PaneNode);
      const { collapsedFrom: _projection, ...stack } = node;
      return { ...stack, id, panes, activePaneId: node.activePaneId ? renamed.get(node.activePaneId) : undefined };
    }
    const sessionId = paneString(node, "session_id");
    const session = sessions.find(session => session.id === sessionId);
    const workspaceId = paneString(node, "workspace_id") ?? session?.workspace_id ?? ownerId;
    const metadata = { ...node.metadata, workspace_id: workspaceId };
    const path = paneString(node, "path");
    const preferred = sessionId ? `session-${sessionId}` : path ? filePane(workspaceId, path).id : node.kind === "git_diff" ? changesPane(workspaceId).id : node.id;
    const id = unique(preferred); renamed.set(node.id, id);
    return { ...node, id, metadata };
  }
  const result = { version: restored.version, root: visit(restored.root) };
  if (!validateLayout(result)) throw new Error("Invalid migrated layout");
  return result;
}
