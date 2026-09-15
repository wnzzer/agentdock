import type { PaneNode, Session, Workspace } from "@agentdock/protocol";
import { fuzzyScore } from "./fuzzy-search";
import { isSessionArchived } from "./session-list";

export interface WorkspaceGroupPreferences {
  version: 1;
  pinned: string[];
  /** Only explicit user choices; absent keys retain the selected/active default. */
  expanded: Record<string, boolean>;
}

export interface WorkspaceGroup {
  workspace: Workspace;
  sessions: Session[];
  totalSessions: number;
  activeCount: number;
  pinned: boolean;
  expanded: boolean;
  matchesWorkspace: boolean;
}

export interface WorkspaceGroupOptions {
  query?: string;
  selectedWorkspaceId?: string;
  preferences?: WorkspaceGroupPreferences;
  /** Search expansion is temporary and must not overwrite the normal layout. */
  searchExpanded?: Record<string, boolean>;
}

export function emptyWorkspaceGroupPreferences(): WorkspaceGroupPreferences {
  return { version: 1, pinned: [], expanded: Object.create(null) as Record<string, boolean> };
}

const active = (session: Session) => ["starting", "running", "waiting"].includes(session.status);
const providerNames = { claude_code: "Claude Code", codex: "Codex", terminal: "Terminal" };

/** Two levels only: known workspaces and their own sessions. Search stays entirely local. */
export function groupWorkspaces(workspaces: readonly Workspace[], sessions: readonly Session[], options: WorkspaceGroupOptions = {}): WorkspaceGroup[] {
  const preferences = options.preferences ?? emptyWorkspaceGroupPreferences();
  const query = options.query?.trim() ?? "";
  const grouped = new Map<string, Session[]>();
  const seenSessions = new Set<string>();
  for (const session of sessions) {
    if (seenSessions.has(session.id)) continue;
    seenSessions.add(session.id);
    // Archiving is an explicit request to hide a session. A temporary one is
    // the opposite: it is open and in use right now, and it disappears by
    // itself when closed, so hiding it only made it unmanageable.
    if (isSessionArchived(session)) continue;
    const group = grouped.get(session.workspace_id) ?? [];
    group.push(session);
    grouped.set(session.workspace_id, group);
  }
  const pinned = new Map(preferences.pinned.map((id, index) => [id, index]));
  return workspaces.map((workspace, index) => {
    const allSessions = grouped.get(workspace.id) ?? [];
    const activeCount = allSessions.filter(active).length;
    const workspaceScore = fuzzyScore(query, `${workspace.name} ${workspace.root_path} ${workspace.id}`);
    const matchesWorkspace = workspaceScore !== null;
    const matches = allSessions.map((session, sessionIndex) => ({
      session, index: sessionIndex,
      score: fuzzyScore(query, `${session.title} ${session.id} ${providerNames[session.provider]} ${session.provider} ${workspace.name} ${workspace.root_path} ${session.status}`),
    })).filter((item): item is { session: Session; index: number; score: number } => item.score !== null)
      .sort((a, b) => a.score - b.score || a.index - b.index);
    const visibleSessions = matchesWorkspace ? allSessions.slice() : matches.map(item => item.session);
    const defaultExpanded = workspace.id === options.selectedWorkspaceId || activeCount > 0;
    const normalExpanded = Object.hasOwn(preferences.expanded, workspace.id) ? preferences.expanded[workspace.id] : defaultExpanded;
    const expanded = query ? (options.searchExpanded && Object.hasOwn(options.searchExpanded, workspace.id) ? options.searchExpanded[workspace.id] : true) : normalExpanded;
    const score = Math.min(workspaceScore ?? Infinity, matches[0]?.score ?? Infinity);
    const group: WorkspaceGroup = { workspace, sessions: visibleSessions, totalSessions: allSessions.length, activeCount, pinned: pinned.has(workspace.id), expanded, matchesWorkspace };
    return { group, index, score };
  }).filter(item => !query || item.group.matchesWorkspace || item.group.sessions.length)
    .sort((a, b) => Number(b.group.pinned) - Number(a.group.pinned)
      || (a.group.pinned && b.group.pinned ? pinned.get(a.group.workspace.id)! - pinned.get(b.group.workspace.id)! : 0)
      || (query ? a.score - b.score : 0) || a.index - b.index)
    .map(item => item.group);
}

export function workspaceSessionPane(session: Session): PaneNode {
  return {
    type: "pane", id: `session-${session.id}`, kind: session.provider === "terminal" ? "terminal" : "agent_chat", title: session.title,
    metadata: { workspace_id: session.workspace_id, session_id: session.id, provider: session.provider },
  };
}

type PreferenceStorage = Pick<Storage, "getItem" | "setItem">;
const memory = new Map<string, WorkspaceGroupPreferences>();
const validId = (value: unknown): value is string => typeof value === "string" && value.length > 0 && value.length <= 256 && !value.includes("\0");
const copyPreferences = (value: WorkspaceGroupPreferences): WorkspaceGroupPreferences => ({ version: 1, pinned: [...value.pinned], expanded: Object.assign(Object.create(null), value.expanded) });

export function parseWorkspaceGroupPreferences(value: unknown): WorkspaceGroupPreferences {
  const result = emptyWorkspaceGroupPreferences();
  if (!value || typeof value !== "object" || Array.isArray(value)) return result;
  const record = value as Record<string, unknown>;
  if (record.version !== 1) return result;
  if (Array.isArray(record.pinned)) result.pinned = [...new Set(record.pinned.filter(validId))].slice(0, 2000);
  if (record.expanded && typeof record.expanded === "object" && !Array.isArray(record.expanded)) {
    for (const [id, expanded] of Object.entries(record.expanded).slice(0, 2000)) if (validId(id) && typeof expanded === "boolean") result.expanded[id] = expanded;
  }
  return result;
}

export function workspaceGroupStorageKey(deployment: string): string {
  return `agentdock.workspace-groups.v1:${encodeURIComponent(deployment)}`;
}

/** Persist only opaque IDs, not paths/titles. Denied/quota-limited storage stays usable in memory. */
export function loadWorkspaceGroupPreferences(key: string, storage?: PreferenceStorage): WorkspaceGroupPreferences {
  const remembered = memory.get(key);
  if (remembered) return copyPreferences(remembered);
  let preferences = emptyWorkspaceGroupPreferences();
  try {
    const raw = storage?.getItem(key);
    if (raw && raw.length <= 256_000) preferences = parseWorkspaceGroupPreferences(JSON.parse(raw));
  } catch { /* Corrupt or unavailable storage starts with safe defaults. */ }
  memory.set(key, copyPreferences(preferences));
  return preferences;
}

export function saveWorkspaceGroupPreferences(key: string, value: WorkspaceGroupPreferences, storage?: PreferenceStorage): void {
  const preferences = parseWorkspaceGroupPreferences(value);
  memory.set(key, copyPreferences(preferences));
  try { storage?.setItem(key, JSON.stringify(preferences)); } catch { /* Memory retains the user's choices for this page lifetime. */ }
}
