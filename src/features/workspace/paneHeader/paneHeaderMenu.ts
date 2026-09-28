import type { TabMenuEntry } from "../strip/tabCloseMenu";

/** Do not add a file named PaneHeaderMenu.* beside this one: on a
 * case-insensitive filesystem it collides with this module (TS1149). */
export interface HeaderMenuSeam {
  closeEntries: TabMenuEntry[];
  onCloseEntry: (key: TabMenuEntry["key"]) => void;
  /** Opens the rename dialog for this header's session. Absent when the
   * daemon does not advertise the capability that gates the frame — the
   * entry is hidden, never disabled. */
  onRename?: (() => void) | null;
}

export interface PaneHeaderMenu {
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

/** The header menu with no path row when the row carries no cwd, and no
 * close rows until the workspace wires the seam: entry enablement needs the
 * tab's roster position and firing needs the tab-close flow, both above the
 * surfaces. Null means no kebab — a menu with nothing actionable is dead;
 * a Rename entry alone is actionable. */
export function headerMenu(
  cwd: string | undefined,
  seam: HeaderMenuSeam | undefined,
): PaneHeaderMenu | null {
  const copyPath = !cwd ? null : cwd;
  const closeEntries = seam?.closeEntries ?? [];
  const onRename = seam?.onRename ?? null;
  if (copyPath === null && closeEntries.length === 0 && onRename === null) return null;
  return { copyPath, closeEntries, onCloseEntry: seam?.onCloseEntry ?? null, onRename };
}
