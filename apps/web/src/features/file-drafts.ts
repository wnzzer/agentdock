import { reactive } from "vue";

export interface FileDraft { content: string; original: string; version: string; loaded: boolean; }
/** Drafts survive tab/split moves. They are deliberately not written to localStorage. */
const drafts = reactive(new Map<string, FileDraft>());
export function fileDraft(workspaceId: string, path: string): FileDraft {
  const key = `${workspaceId}:${path}`;
  if (!drafts.has(key)) drafts.set(key, { content: "", original: "", version: "", loaded: false });
  return drafts.get(key)!;
}
export function hasDirtyDrafts(workspaceId?: string): boolean {
  return [...drafts.entries()].some(([key, value]) => (!workspaceId || key.startsWith(`${workspaceId}:`)) && value.content !== value.original);
}
