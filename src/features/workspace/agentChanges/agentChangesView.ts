import type { WorkspaceAgentEditFile } from "../../../types/ipc";

/** The files in path order: a list that holds still while an agent keeps editing. */
export function sortedFiles(files: readonly WorkspaceAgentEditFile[]): WorkspaceAgentEditFile[] {
  return [...files].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

/** A path's last segment and its folder; the folder is empty for a file at the root. */
export function splitPath(path: string): { name: string; dir: string } {
  const slash = path.lastIndexOf("/");
  return slash < 0
    ? { name: path, dir: "" }
    : { name: path.slice(slash + 1), dir: path.slice(0, slash) };
}

/** The file count; a list the daemon cut short reads with a plus. */
export function filesText(count: number, truncated = false): string {
  const noun = count === 1 ? "file" : "files";
  return truncated ? `${count}+ ${noun}` : `${count} ${noun}`;
}

/** An edit count; a call list the daemon cut short reads with a plus. */
export function editsText(count: number, truncated = false): string {
  const noun = count === 1 ? "edit" : "edits";
  return truncated ? `${count}+ ${noun}` : `${count} ${noun}`;
}

/** How long ago an agent's edit landed, in minutes, hours or days. */
export function editedAgo(atMs: number, now: number): string {
  const minutes = Math.floor(Math.max(0, now - atMs) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  return `${Math.floor(hours / 24)} d ago`;
}
