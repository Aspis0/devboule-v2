/**
 * The workspace-relative spelling of a path, or null when the path is not inside
 * the workspace root. Both sides are normalised first: separators, `.` and `..`.
 * The inside test is on whole path components, so `/a/test2/x` is not inside
 * `/a/test`. A drive letter or a Windows folder compares without case; a POSIX
 * root compares with it.
 *
 * A home path (`~`), a UNC path (`//server`), and an absolute path that the root
 * cannot contain are outside, since the frontend cannot tell where they resolve.
 */

// A `:line` or `:line:col` suffix names a location in the file, not part of its name.
const LOCATION_SUFFIX = /:\d+(?::\d+)?$/;
const DRIVE = /^([A-Za-z]):/;
const UNC = /^[\\/]{2}/;
// The Windows verbatim prefix names the same drive path; a verbatim UNC share stays a share.
const VERBATIM_DRIVE = /^(?:\\\\\?\\|\/\/\?\/)(?=[A-Za-z]:)/;

function withoutVerbatim(path: string): string {
  return path.replace(VERBATIM_DRIVE, "");
}

interface Parts {
  drive: string | null;
  absolute: boolean;
  parts: string[];
}

/** Splits a path into its drive, whether it is absolute, and its components; null when `..` climbs past the start. */
function componentsOf(path: string): Parts | null {
  const slashed = path.replace(/\\/g, "/");
  const drive = DRIVE.exec(slashed);
  const rest = drive === null ? slashed : slashed.slice(2);
  const parts: string[] = [];
  for (const part of rest.split("/")) {
    if (part === "" || part === ".") continue;
    if (part !== "..") {
      parts.push(part);
    } else if (parts.length === 0) {
      return null;
    } else {
      parts.pop();
    }
  }
  return {
    drive: drive === null ? null : drive[1]!.toUpperCase(),
    absolute: drive !== null || slashed.startsWith("/"),
    parts,
  };
}

export function workspaceRelativePath(path: string, workspaceRoot: string): string | null {
  const target = withoutVerbatim(path.replace(LOCATION_SUFFIX, ""));
  const root = withoutVerbatim(workspaceRoot);
  if (target.startsWith("~") || UNC.test(target) || UNC.test(root)) return null;
  // `C:folder/x` names a place relative to the drive's working folder, not the root.
  if (DRIVE.test(target) && !/^[A-Za-z]:[\\/]/.test(target)) return null;
  const workspace = componentsOf(root);
  const spelled = componentsOf(target);
  if (workspace === null || !workspace.absolute || spelled === null) return null;
  // An absolute target must share the root's drive (or both be drive-less).
  if (spelled.absolute && spelled.drive !== workspace.drive) return null;
  const parts = spelled.absolute ? spelled.parts : [...workspace.parts, ...spelled.parts];
  if (parts.length <= workspace.parts.length) return null;
  const foldCase = workspace.drive !== null;
  const sameRoot = workspace.parts.every((part, index) => {
    const other = parts[index]!;
    return foldCase ? part.toLowerCase() === other.toLowerCase() : part === other;
  });
  return sameRoot ? parts.slice(workspace.parts.length).join("/") : null;
}
