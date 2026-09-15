import { computed, ref, watch, type ComputedRef, type Ref } from "vue";
import {
  projectLayout,
  createDefaultLayoutDocument,
  findNodes,
  normalizeRatio,
  resizeSplit,
  restoreCollapsed,
  splitPane,
  validateLayout,
  type LayoutDocument,
  type LayoutNode,
  type SplitDirection,
} from "./layout-engine";

export interface UseLayoutEngineOptions {
  /** localStorage key. Omit to keep the layout in memory only. */
  key?: string;
  initial?: LayoutDocument | LayoutNode;
  storage?: Storage;
  serialize?: (document: LayoutDocument) => string;
  deserialize?: (value: string) => LayoutDocument | LayoutNode | null;
  /** Called after a user action changes the persisted layout. */
  onLayoutChange?: (document: LayoutDocument) => void;
  /** Short alias retained for callers that use a generic change callback. */
  onChange?: (document: LayoutDocument) => void;
}

export interface LayoutEngineActions {
  setDocument(document: LayoutDocument): void;
  selectPane(id: string | null): void;
  splitPane(id: string, direction: SplitDirection, ratio?: number): void;
  resizeSplit(id: string, ratio: number): void;
  collapseBelowMin(width: number, height: number, minW?: number, minH?: number): void;
  restoreCollapsed(splitId?: string): void;
  reset(): void;
  findNodes(matcher?: string | ((node: LayoutNode) => boolean)): LayoutNode[];
  isValid(): boolean;
}

export interface LayoutEngine {
  doc: Ref<LayoutDocument>;
  /** Disposable responsive view. Persist doc, never view. */
  view: ComputedRef<LayoutNode>;
  selectedPane: Ref<string | null>;
  actions: LayoutEngineActions;
}

function asDocument(value: LayoutDocument | LayoutNode | null | undefined): LayoutDocument | undefined {
  if (!value) return undefined;
  if ("root" in value && "version" in value) {
    if (!validateLayout(value.root)) return undefined;
    return {
      version: value.version,
      root: value.root,
      collapsed: value.collapsed,
    };
  }
  if (!validateLayout(value)) return undefined;
  return { version: 1, root: value };
}

function defaultStorage(): Storage | undefined {
  if (typeof globalThis === "undefined" || !("localStorage" in globalThis)) return undefined;
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

/**
 * Vue adapter for the pure layout engine. Every action replaces `doc.value`
 * with a new tree, which makes undo/history and persistence straightforward.
 */
export function useLayoutEngine(optionsOrKey: UseLayoutEngineOptions | string = {}): LayoutEngine {
  const options: UseLayoutEngineOptions = typeof optionsOrKey === "string" ? { key: optionsOrKey } : optionsOrKey;
  const storage = options.storage ?? (options.key ? defaultStorage() : undefined);
  const serialize = options.serialize ?? ((document: LayoutDocument) => JSON.stringify(document));
  const deserialize = options.deserialize ?? ((value: string) => JSON.parse(value) as LayoutDocument | LayoutNode);

  let initial = asDocument(options.initial);
  if (!initial && options.key && storage) {
    try {
      const raw = storage.getItem(options.key);
      if (raw) initial = asDocument(deserialize(raw));
    } catch {
      // A corrupt or inaccessible preference should never prevent the app from loading.
    }
  }
  const doc = ref<LayoutDocument>(restoreCollapsed(initial ?? createDefaultLayoutDocument()));
  const selectedPane = ref<string | null>(null);
  const viewport = ref<{ width: number; height: number; minWidth: number; minHeight: number } | null>(null);
  const view = computed(() => viewport.value ? projectLayout(doc.value.root, viewport.value.width, viewport.value.height, {
    minWidth: viewport.value.minWidth, minHeight: viewport.value.minHeight, selectedPaneId: selectedPane.value,
  }) : doc.value.root);

  const persistAndNotify = (document: LayoutDocument) => {
    if (options.key && storage) {
      try {
        storage.setItem(options.key, serialize(document));
      } catch {
        // Storage quotas and private browsing are allowed to fail silently.
      }
    }
    options.onLayoutChange?.(document);
    options.onChange?.(document);
  };

  watch(doc, (document) => persistAndNotify(document), { deep: true, flush: "sync" });

  const actions: LayoutEngineActions = {
    setDocument(document) {
      if (!validateLayout(document)) return;
      doc.value = restoreCollapsed(document);
    },
    selectPane(id) {
      selectedPane.value = id;
    },
    splitPane(id, direction, ratio = 0.5) {
      doc.value = { ...doc.value, root: splitPane(doc.value.root, id, direction, normalizeRatio(ratio)) };
    },
    resizeSplit(id, ratio) {
      doc.value = { ...doc.value, root: resizeSplit(doc.value.root, id, normalizeRatio(ratio)) };
    },
    collapseBelowMin(width, height, minW = 280, minH = 180) {
      viewport.value = { width, height, minWidth: minW, minHeight: minH };
    },
    restoreCollapsed(splitId) {
      doc.value = restoreCollapsed(doc.value, splitId);
      viewport.value = null;
    },
    reset() {
      doc.value = createDefaultLayoutDocument();
      selectedPane.value = null;
    },
    findNodes(matcher) {
      return findNodes(doc.value.root, matcher);
    },
    isValid() {
      return validateLayout(doc.value.root);
    },
  };

  return { doc, view, selectedPane, actions };
}
