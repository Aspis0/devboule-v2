import type { WorkspaceDirectory } from "../../types/ipc";

/** Most file paths one index holds: the "@" menu ranks over them on each key. */
export const FILE_INDEX_MAX_FILES = 5_000;

/**
 * Most folders one index lists. Each folder is one listing call to the daemon,
 * so a large tree costs a bounded number of calls and the index stops at the cap
 * rather than walking the whole disk.
 */
export const FILE_INDEX_MAX_DIRECTORIES = 120;

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

/**
 * The workspace's file paths, breadth-first from the root, relative and
 * `/`-separated like the Files panel's own keys. A folder the daemon refuses,
 * or a listing that throws, is skipped: the index is a menu, and a missing
 * folder costs one entry, never the whole index.
 */
export async function indexWorkspaceFiles(list: ListDirectory): Promise<string[]> {
  const files: string[] = [];
  const queue: string[] = [""];
  let next = 0;
  let listed = 0;
  while (
    next < queue.length &&
    files.length < FILE_INDEX_MAX_FILES &&
    listed < FILE_INDEX_MAX_DIRECTORIES
  ) {
    const folder = queue[next];
    next += 1;
    listed += 1;
    let reply: WorkspaceDirectory;
    try {
      reply = await list(folder);
    } catch {
      continue;
    }
    if (reply.error !== null) continue;
    for (const entry of reply.entries) {
      if (entry.kind === "dir") {
        if (!SKIPPED_DIRECTORIES.has(entry.name)) queue.push(entry.path);
      } else if (files.length < FILE_INDEX_MAX_FILES) {
        files.push(entry.path);
      }
    }
  }
  return files;
}

/** How long one built index serves the "@" menus before a new "@" rebuilds it. */
export const FILE_INDEX_TTL_MS = 60_000;

interface BuiltIndex {
  files: Promise<string[]>;
  builtAt: number;
}

/**
 * One index per workspace, reused by the "@" menus for a minute: a file
 * created meanwhile is found by the next menu after that, and a quiet
 * workspace never pays for the walk twice.
 */
const indexes = new Map<string, BuiltIndex>();

export function workspaceFileIndex(workspaceId: string, list: ListDirectory): Promise<string[]> {
  const existing = indexes.get(workspaceId);
  if (existing !== undefined && Date.now() - existing.builtAt < FILE_INDEX_TTL_MS) {
    return existing.files;
  }
  const files = indexWorkspaceFiles(list);
  indexes.set(workspaceId, { files, builtAt: Date.now() });
  return files;
}

/** Test seam: forget every built index. */
export function resetWorkspaceFileIndexesForTests(): void {
  indexes.clear();
}
