import type { TabMenuEntry } from "../strip/tabCloseMenu";
import { displayPath } from "../../../lib/displayPath";

/** Do not add a file named PaneHeaderMenu.* beside this one: on a
 * case-insensitive filesystem it collides with this module (TS1149). */
export interface HeaderMenuSeam {
  workspaceId?: string | null;
  closeEntries: TabMenuEntry[];
  onCloseEntry: (key: TabMenuEntry["key"]) => void;
  /** Opens the rename dialog for this header's session. Absent when the
   * daemon does not advertise the capability that gates the frame — the
   * entry is hidden, never disabled. */
  onRename?: (() => void) | null;
}

export interface PaneHeaderMenu {
  workspaceId?: string | null;
  copyBranchName?: string | null;
  copySessionId: string | null;
  copyPath: string | null;
  closeEntries: TabMenuEntry[];
  onCloseEntry: ((key: TabMenuEntry["key"]) => void) | null;
  onRename: (() => void) | null;
}

const PATH_NOTE_LIMIT = 28;

function splitClusters(value: string): string[] {
  const Segmenter = Intl.Segmenter;
  if (typeof Segmenter === "function") {
    return Array.from(
      new Segmenter("en", { granularity: "grapheme" }).segment(value),
      (part) => part.segment,
    );
  }
  return Array.from(value);
}

/** Middle-truncates a path for the menu's note row, clusters not code units:
 * a unit-based cut halves an astral scalar and renders U+FFFD. The limit is
 * set against the note's own box (240 px menu, 206 px of note): at 13 px the
 * average cluster advances ~6.8 px, so 28 clusters fit with room, and the
 * row carries no CSS ellipsis that could eat the preserved tail. */
export function middleTruncate(value: string, maxLength = PATH_NOTE_LIMIT): string {
  const clusters = splitClusters(value);
  if (clusters.length <= maxLength) return value;
  const keep = maxLength - 1;
  const head = Math.ceil(keep / 2);
  const tail = keep - head;
  return `${clusters.slice(0, head).join("")}…${clusters.slice(clusters.length - tail).join("")}`;
}

/** Null hides the kebab when the surface has no available action;
 * a copy or Rename row alone is actionable. */
export function headerMenu(
  cwd: string | undefined,
  seam: HeaderMenuSeam | undefined,
  sessionId?: string,
): PaneHeaderMenu | null {
  const copySessionId = sessionId || null;
  const copyPath = displayPath(cwd ?? "") || null;
  const closeEntries = seam?.closeEntries ?? [];
  const onRename = seam?.onRename ?? null;
  if (copySessionId === null && copyPath === null && closeEntries.length === 0 && onRename === null)
    return null;
  return {
    workspaceId: seam?.workspaceId ?? null,
    copyBranchName: null,
    copySessionId,
    copyPath,
    closeEntries,
    onCloseEntry: seam?.onCloseEntry ?? null,
    onRename,
  };
}
