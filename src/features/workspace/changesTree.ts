import type { WorkspaceGitRow } from "../../types/ipc";

/**
 * The flat status rows grouped into a folder tree with aggregated stats.
 * One O(n) pass over the reply's rows: the daemon's order is the display
 * order, folders opening in first-appearance position, files and folders
 * interleaved as they arrive — nothing here sorts, the wire stays the
 * authority. Each folder carries its subtree's FULL sums, collapsed or
 * not (Paseo's `diff-tree.ts` keeps the same guarantee), and any `capped`
 * row in the subtree marks every ancestor: those sums are floors.
 */
export interface ChangesTreeFile {
  kind: "file";
  /** The row's own workspace-relative path — what acts key on. */
  path: string;
  /** The last segment — what the row shows, the full path in `title`. */
  name: string;
  row: WorkspaceGitRow;
}

export interface ChangesTreeFolder {
  kind: "folder";
  /** The folder's workspace-relative path, e.g. `src/checkout`. */
  path: string;
  name: string;
  children: ChangesTreeNode[];
  additions: number;
  deletions: number;
  /** True when any row in the subtree is capped. */
  capped: boolean;
}

export type ChangesTreeNode = ChangesTreeFile | ChangesTreeFolder;

function basename(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? path : path.slice(slash + 1);
}

export function buildChangesTree(rows: WorkspaceGitRow[]): ChangesTreeNode[] {
  const top: ChangesTreeNode[] = [];
  const folders = new Map<string, ChangesTreeFolder>();

  const folderAt = (path: string, parent: ChangesTreeNode[]): ChangesTreeFolder => {
    const known = folders.get(path);
    if (known !== undefined) return known;
    const folder: ChangesTreeFolder = {
      kind: "folder",
      path,
      name: basename(path),
      children: [],
      additions: 0,
      deletions: 0,
      capped: false,
    };
    folders.set(path, folder);
    parent.push(folder);
    return folder;
  };

  for (const row of rows) {
    const slash = row.path.lastIndexOf("/");
    if (slash < 0) {
      top.push({ kind: "file", path: row.path, name: row.path, row });
      continue;
    }
    const segments = row.path.split("/");
    let parent = top;
    let prefix = "";
    for (const segment of segments.slice(0, -1)) {
      prefix = prefix === "" ? segment : `${prefix}/${segment}`;
      parent = folderAt(prefix, parent).children;
    }
    parent.push({ kind: "file", path: row.path, name: basename(row.path), row });
  }

  // Post-order sums, leaves first: each folder's totals are its own files
  // plus every descendant folder's, and one capped row anywhere below
  // marks the whole chain above it.
  const sum = (
    nodes: ChangesTreeNode[],
  ): { additions: number; deletions: number; capped: boolean } => {
    let additions = 0;
    let deletions = 0;
    let capped = false;
    for (const node of nodes) {
      if (node.kind === "file") {
        additions += node.row.additions;
        deletions += node.row.deletions;
        capped = capped || node.row.capped;
      } else {
        const child = sum(node.children);
        node.additions = child.additions;
        node.deletions = child.deletions;
        node.capped = child.capped;
        additions += child.additions;
        deletions += child.deletions;
        capped = capped || child.capped;
      }
    }
    return { additions, deletions, capped };
  };
  sum(top);
  return top;
}
