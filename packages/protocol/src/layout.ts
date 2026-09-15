/**
 * The serialisable layout protocol shared by the web client and (eventually)
 * the server-side workspace registry.  Layout nodes deliberately contain no
 * Vue or DOM state so a document can be persisted and restored as JSON.
 */

export type PaneKind =
  | "agent_chat"
  | "editor"
  | "terminal"
  | "git_diff"
  | "file_preview"
  | "stack";

export type LeafPaneKind = Exclude<PaneKind, "stack">;
export type SplitDirection = "horizontal" | "vertical";

export interface PaneNode {
  type: "pane";
  id: string;
  kind: LeafPaneKind;
  /** A user-facing title is optional; the registry may provide a default. */
  title?: string;
  /** Provider/session/file identifiers are intentionally opaque to layout. */
  metadata?: Record<string, unknown>;
}

export interface StackNode {
  type: "stack";
  /** `kind` makes stacks usable anywhere a PaneKind is displayed. */
  kind: "stack";
  id: string;
  panes: PaneNode[];
  activePaneId?: string;
  /** View-only: canonical split represented by this responsive tab group.
   * Do not persist projected stacks; retain the canonical LayoutDocument. */
  collapsedFrom?: string;
}

export interface SplitNode {
  type: "split";
  id: string;
  direction: SplitDirection;
  /** Ratio of the first child in the range (0, 1). */
  ratio: number;
  first: LayoutNode;
  second: LayoutNode;
}

export type LayoutNode = PaneNode | SplitNode | StackNode;

export interface CollapsedLayout {
  splitId: string;
  replacementId: string;
  node: SplitNode;
}

export interface LayoutDocument {
  version: number;
  root: LayoutNode;
  /** Legacy snapshot storage; read for migration only. New layouts keep the
   * canonical split tree and derive responsive tabs without changing it. */
  collapsed?: CollapsedLayout[];
}
