import type { WorkspaceFileEntry } from "../../types/ipc";

/**
 * The panel collation: `en`, base sensitivity, numeric. Case folds (Zeta
 * sorts after alpha, and `A.txt` equals `a.txt`), numbers run naturally
 * (`a2` before `a10`), accents fold to their base (éclair with e) — the
 * human order the daemon's byte order is not, stated here so the ordering
 * test pins a name rather than an accident.
 */
const NAME_ORDER = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

/**
 * One comparison: folders before files, always — then names in the panel
 * collation. A second criterion arrives with the daemon data that can
 * order it (file times, which the wire does not carry); until then there
 * is one order and the toolbar names it instead of offering a menu.
 */
export function compareFileEntries(a: WorkspaceFileEntry, b: WorkspaceFileEntry): number {
  if (a.kind !== b.kind) return a.kind === "dir" ? -1 : 1;
  return NAME_ORDER.compare(a.name, b.name);
}

/** The reply's entries in the panel's order — a copy, never sorted in place. */
export function sortFileEntries(entries: readonly WorkspaceFileEntry[]): WorkspaceFileEntry[] {
  return [...entries].sort(compareFileEntries);
}
