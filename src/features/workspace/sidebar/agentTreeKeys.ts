/** What the tree needs of a row to move focus across it. */
export interface TreeNode {
  id: string;
  parentId: string | null;
  hasChildren: boolean;
}

/**
 * The row a key moves focus to, or null when the key does not move it. The
 * first child of a row follows the row directly, so Right enters it by index.
 */
export function treeKeyTarget(
  key: string,
  index: number,
  nodes: readonly TreeNode[],
): number | null {
  const last = nodes.length - 1;
  switch (key) {
    case "ArrowDown":
      return index < last ? index + 1 : null;
    case "ArrowUp":
      return index > 0 ? index - 1 : null;
    case "Home":
      return index > 0 ? 0 : null;
    case "End":
      return index < last ? last : null;
    case "ArrowRight": {
      const node = nodes[index];
      return node?.hasChildren === true ? index + 1 : null;
    }
    case "ArrowLeft": {
      const parentId = nodes[index]?.parentId ?? null;
      if (parentId === null) return null;
      const parent = nodes.findIndex((node) => node.id === parentId);
      return parent < 0 ? null : parent;
    }
    default:
      return null;
  }
}

/**
 * Moves focus from a workspace row's wrapper into the agent tree drawn right
 * after it, and answers whether the tree took focus.
 */
export function focusAgentTreeAfter(workspaceRowWrap: HTMLElement | null): boolean {
  const first = workspaceRowWrap?.nextElementSibling?.querySelector<HTMLElement>(
    '[role="treeitem"][tabindex="0"]',
  );
  if (first === null || first === undefined) return false;
  first.focus();
  return true;
}

/** The workspace row an agent block hangs under, or null when none is drawn. */
export function workspaceRowAbove(agentRows: HTMLElement): HTMLElement | null {
  return (
    agentRows.previousElementSibling?.querySelector<HTMLElement>("button.workspace-row") ?? null
  );
}

/** Moves focus from the agent tree back to the workspace row it hangs under. */
export function focusWorkspaceRowAbove(agentRows: HTMLElement): boolean {
  const row = workspaceRowAbove(agentRows);
  if (row === null) return false;
  row.focus();
  return true;
}
