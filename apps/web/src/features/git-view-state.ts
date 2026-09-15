import { reactive } from "vue";

export interface GitViewState { message: string; selected?: { path: string; staged: boolean }; }
const views = reactive(new Map<string, GitViewState>());
/** UI drafts must survive the layout projecting a split into tabs. */
export function gitViewState(workspaceId: string): GitViewState {
  if (!views.has(workspaceId)) views.set(workspaceId, { message: "" });
  return views.get(workspaceId)!;
}
export function hasGitDrafts() { return [...views.values()].some(view => view.message.trim().length > 0); }
