/**
 * The local path a markdown link's destination names, or null when the destination
 * is not a path at all: a web or other URL, a file URL that names another machine,
 * or a control character. Deciding whether the path is inside the workspace belongs
 * to workspaceRelativePath; this only cleans the spelling the markdown carries.
 */

const SCHEME = /^([a-z][a-z0-9+.-]*):/i;
// A `#L12`, `#L12C3` or `#L12-L20` fragment names a line; the path is what's left.
const LINE_FRAGMENT = /#L\d+(?:C\d+)?(?:-L\d+(?:C\d+)?)?$/i;
// A control character cannot be a path, and it would hide a scheme from the check below.
const CONTROL_CHARACTER = /[\u0000-\u001f\u007f]/;

// Well-known file names with no extension; any other name with no extension is taken for a folder.
const EXTENSIONLESS_FILES = new Set(["Dockerfile", "LICENSE", "Makefile", "NOTICE", "VERSION"]);
const SEPARATOR_END = /[\\/]$/;
const EXTENSION = /\.[^./\\]+$/;

/**
 * Whether a local path names a folder. The frontend cannot ask the disk, so a path
 * that ends in a separator, or whose last name has no extension, is taken for one.
 */
export function isFolderTarget(path: string): boolean {
  if (SEPARATOR_END.test(path)) return true;
  const name = path.split(/[\\/]/).pop() ?? "";
  return !EXTENSION.test(name) && !EXTENSIONLESS_FILES.has(name);
}

export function localPathFromHref(href: string): string | null {
  if (CONTROL_CHARACTER.test(href)) return null;
  let target = href.trim();
  if (target.startsWith("<") && target.endsWith(">")) target = target.slice(1, -1);
  if (/^file:\/\/\//i.test(target)) {
    // `file:///C:/x` carries a slash before its drive letter; a path does not.
    target = target.slice("file://".length).replace(/^\/([A-Za-z]:)/, "$1");
  } else if (/^file:/i.test(target)) {
    // `file://host/share` is another machine's share: its UNC spelling, never a path here.
    target = target.slice("file:".length);
  } else {
    const scheme = SCHEME.exec(target);
    // A one-letter scheme is a drive, not a URL: `C:/x` is a path.
    if (scheme !== null && scheme[1]!.length > 1) return null;
  }
  try {
    target = decodeURI(target);
  } catch {
    // A malformed escape stays as the author wrote it.
  }
  target = target.replace(LINE_FRAGMENT, "");
  return target === "" ? null : target;
}
