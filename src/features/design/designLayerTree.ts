import type { DesignLayer } from "./designHost";

export function isHidden(hiddenLayerIds: readonly string[], layerId: string): boolean {
  return hiddenLayerIds.includes(layerId);
}

/** One direction of tree movement while a layer is selected. */
export type LayerMove = "parent" | "first-child" | "previous-sibling" | "next-sibling";

/**
 * The parent/child shape of the displayed layers, built once from
 * `section.parentId` (see `ArtifactSection.parent`). Canvas layers carry no
 * `section`, so they are roots; page-section roots sit at the same level. The
 * keys are layer ids, never positions: `displayLayers` concatenates the canvas
 * layers ahead of the measured sections, so an index into that list would point
 * at the wrong layer the moment it is filtered or reordered.
 */
export interface LayerTree {
  readonly roots: readonly DesignLayer[];
  readonly byId: ReadonlyMap<string, DesignLayer>;
  readonly parentOf: ReadonlyMap<string, string>;
  readonly childrenOf: ReadonlyMap<string, readonly DesignLayer[]>;
}

/**
 * Builds the tree in document order. A duplicate id keeps its first occurrence,
 * matching `Map` semantics. A `parentId` that resolves to no layer in the list
 * (only possible when a caller hands over a filtered list) reads as a root, so
 * a child is never stranded: the child stays reachable even if its parent was
 * left out.
 */
export function buildLayerTree(layers: readonly DesignLayer[]): LayerTree {
  const byId = new Map<string, DesignLayer>();
  for (const layer of layers) {
    if (!byId.has(layer.id)) byId.set(layer.id, layer);
  }
  const parentOf = new Map<string, string>();
  const childLists = new Map<string, DesignLayer[]>();
  const roots: DesignLayer[] = [];
  for (const layer of byId.values()) {
    const parentId = layer.section?.parentId;
    if (parentId === undefined || !byId.has(parentId)) {
      roots.push(layer);
      continue;
    }
    parentOf.set(layer.id, parentId);
    const siblings = childLists.get(parentId);
    if (siblings === undefined) childLists.set(parentId, [layer]);
    else siblings.push(layer);
  }
  return { roots, byId, parentOf, childrenOf: childLists };
}

/** Root-to-leaf ids for the given layer, inclusive; empty when it is unknown. */
export function layerAncestorIds(tree: LayerTree, layerId: string): readonly string[] {
  const chain: string[] = [];
  const seen = new Set<string>();
  let current: string | null = tree.byId.has(layerId) ? layerId : null;
  while (current !== null && !seen.has(current)) {
    seen.add(current);
    chain.push(current);
    current = tree.parentOf.get(current) ?? null;
  }
  return chain.reverse();
}

/** Root-to-leaf layers for the given layer, inclusive; the breadcrumb source. */
export function layerAncestorChain(tree: LayerTree, layerId: string): readonly DesignLayer[] {
  const chain: DesignLayer[] = [];
  for (const id of layerAncestorIds(tree, layerId)) {
    const layer = tree.byId.get(id);
    if (layer !== undefined) chain.push(layer);
  }
  return chain;
}

/** The id a move would select, or null when the move has nowhere to go. */
export function layerMoveTarget(tree: LayerTree, layerId: string, move: LayerMove): string | null {
  const parentId = tree.parentOf.get(layerId) ?? null;
  if (move === "parent") return parentId;
  if (move === "first-child") {
    const children = tree.childrenOf.get(layerId);
    return children !== undefined && children.length > 0 ? children[0].id : null;
  }
  const siblings = parentId === null ? tree.roots : (tree.childrenOf.get(parentId) ?? []);
  const index = siblings.findIndex((layer) => layer.id === layerId);
  if (index < 0) return null;
  if (move === "previous-sibling") return index > 0 ? siblings[index - 1].id : null;
  return index + 1 < siblings.length ? siblings[index + 1].id : null;
}

/**
 * Arrow keys move through the tree once a layer is selected: Up to the parent,
 * Down to the first child, Left/Right to the previous/next sibling. The canvas
 * binds no arrow key — its pan is pointer drag and its zoom the wheel — so
 * nothing here is taken from it. The shared shell pages surfaces with
 * ArrowLeft/ArrowRight only while the crescent nav is open, and this listener
 * lives on the design surface, so it fires only when focus is already inside
 * the surface; stopping propagation there is what keeps a hover-opened nav from
 * handling the same key twice.
 */
export const LAYER_MOVE_BY_ARROW: ReadonlyMap<string, LayerMove> = new Map<string, LayerMove>([
  ["ArrowUp", "parent"],
  ["ArrowDown", "first-child"],
  ["ArrowLeft", "previous-sibling"],
  ["ArrowRight", "next-sibling"],
]);
