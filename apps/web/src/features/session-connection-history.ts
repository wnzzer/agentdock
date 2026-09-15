import { reactive } from "vue";
import type { NativeHistoryItem } from "@agentdock/protocol";

/** Consent is tied to one displayed history item, source and workspace. */
export function createHistorySelection() {
  const state = reactive<{
    selected?: { workspaceId: string; sourceId: string; item: NativeHistoryItem };
    confirmed: boolean;
  }>({ confirmed: false });
  function clear() { state.selected = undefined; state.confirmed = false; }
  function select(workspaceId: string, sourceId: string, item: NativeHistoryItem) {
    state.selected = { workspaceId, sourceId, item };
    state.confirmed = false;
  }
  function payload(workspaceId: string, sourceId: string) {
    const target = state.selected;
    if (!state.confirmed || !target || target.workspaceId !== workspaceId || target.sourceId !== sourceId) return undefined;
    return { source_id: sourceId, native_id: target.item.id, confirmed_original_config: true as const };
  }
  return { state, clear, select, payload };
}
