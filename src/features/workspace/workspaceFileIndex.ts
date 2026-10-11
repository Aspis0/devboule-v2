import type { WorkspaceDirectory } from "../../types/ipc";

/** Most file paths one index holds: the "@" menu ranks over them on each key. */
export const FILE_INDEX_MAX_FILES = 5_000;

/**
 * Most folders one index lists. Each folder is one listing call to the daemon,
 * so a large tree costs a bounded number of calls and the index stops at the cap
 * rather than walking the whole disk.
 */
export const FILE_INDEX_MAX_DIRECTORIES = 120;

/** Folder listings in flight at once while the index is built. */
export const FILE_INDEX_CONCURRENCY = 4;

/** Folders the "@" index never descends into: tool state and build output. */
const SKIPPED_DIRECTORIES: ReadonlySet<string> = new Set([
  ".git",
  "node_modules",
  "target",
  "dist",
  "build",
  ".next",
]);

export type ListDirectory = (path: string) => Promise<WorkspaceDirectory>;

/** What the "@" menu can offer. `truncated` is true when a cap stopped the walk,
 * so files past it exist that the menu does not list. */
export interface WorkspaceFileIndex {
  files: string[];
  truncated: boolean;
}

/**
 * The workspace's file paths, breadth-first from the root, relative and
 * `/`-separated like the Files panel's own keys. Folders are listed a few at a
 * time, and the results are taken in queue order, so the index is the same
 * however the replies interleave. A folder the daemon refuses, or a listing
 * that throws, is skipped: a missing folder costs one entry, never the index.
 */
export async function indexWorkspaceFiles(list: ListDirectory): Promise<WorkspaceFileIndex> {
  const files: string[] = [];
  const queue: string[] = [""];
  let next = 0;
  let listed = 0;
  let truncated = false;
  while (next < queue.length) {
    if (files.length >= FILE_INDEX_MAX_FILES || listed >= FILE_INDEX_MAX_DIRECTORIES) {
      truncated = true;
      break;
    }
    const batch: string[] = [];
    while (
      batch.length < FILE_INDEX_CONCURRENCY &&
      next < queue.length &&
      listed + batch.length < FILE_INDEX_MAX_DIRECTORIES
    ) {
      batch.push(queue[next]);
      next += 1;
    }
    listed += batch.length;
    const replies = await Promise.all(batch.map((folder) => list(folder).catch(() => null)));
    for (const reply of replies) {
      if (reply === null || reply.error !== null) continue;
      for (const entry of reply.entries) {
        if (entry.kind === "dir") {
          if (!SKIPPED_DIRECTORIES.has(entry.name)) queue.push(entry.path);
        } else if (files.length < FILE_INDEX_MAX_FILES) {
          files.push(entry.path);
        } else {
          truncated = true;
        }
      }
    }
  }
  if (next < queue.length) truncated = true;
  return { files, truncated };
}

/** How long one built index serves the "@" menus before a new "@" rebuilds it. */
export const FILE_INDEX_TTL_MS = 60_000;

interface BuiltIndex {
  index: Promise<WorkspaceFileIndex>;
  builtAt: number;
}

/**
 * One index per workspace, reused by the "@" menus for a minute: a file
 * created meanwhile is found by the next menu after that, and a quiet
 * workspace never pays for the walk twice.
 */
const indexes = new Map<string, BuiltIndex>();

export function workspaceFileIndex(
  workspaceId: string,
  list: ListDirectory,
): Promise<WorkspaceFileIndex> {
  const existing = indexes.get(workspaceId);
  if (existing !== undefined && Date.now() - existing.builtAt < FILE_INDEX_TTL_MS) {
    return existing.index;
  }
  const index = indexWorkspaceFiles(list);
  indexes.set(workspaceId, { index, builtAt: Date.now() });
  return index;
}

/** Test seam: forget every built index. */
export function resetWorkspaceFileIndexesForTests(): void {
  indexes.clear();
}
