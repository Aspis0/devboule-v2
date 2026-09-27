import { buildTabCloseEntries, type TabMenuEntry } from "../strip/tabCloseMenu";

/** The close-group wiring the workspace owns. The header cannot build it:
 * entry enablement needs the tab's roster position, and firing needs the
 * tab-close flow with its confirmations — both live above the surfaces. */
export interface HeaderMenuSeam {
  closeEntries: TabMenuEntry[];
  onCloseEntry: (key: TabMenuEntry["key"]) => void;
}

export interface PaneHeaderMenu {
  copyPath: string | null;
  closeEntries: TabMenuEntry[];
  onCloseEntry: ((key: TabMenuEntry["key"]) => void) | null;
}

const PATH_NOTE_LIMIT = 48;

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
 * a unit-based cut halves an astral scalar and renders U+FFFD. */
export function middleTruncate(value: string, maxLength = PATH_NOTE_LIMIT): string {
  const clusters = splitClusters(value);
  if (clusters.length <= maxLength) return value;
  const keep = maxLength - 1;
  const head = Math.ceil(keep / 2);
  const tail = keep - head;
  return `${clusters.slice(0, head).join("")}…${clusters.slice(clusters.length - tail).join("")}`;
}

/** The brief's exact close set, labelled by the tab menu itself: right,
 * others, close. No left (the brief excludes it), no delete, no rename —
 * rename needs a daemon frame that does not exist — and no raw ids. */
function closeGroup(enable: (key: TabMenuEntry["key"]) => boolean): TabMenuEntry[] {
  return buildTabCloseEntries(0, 1)
    .filter((entry) => entry.key === "right" || entry.key === "others" || entry.key === "close")
    .map((entry) => ({ ...entry, disabled: entry.disabled || !enable(entry.key) }));
}

/** No handler anywhere: every close row disabled, so none can misfire. */
export function unwiredCloseEntries(): TabMenuEntry[] {
  return closeGroup(() => false);
}

/** This tab's Close needs no roster position, so it stays enabled. */
function terminalCloseEntries(): TabMenuEntry[] {
  return closeGroup((key) => key === "close");
}

export function agentHeaderMenu(
  cwd: string | undefined,
  seam: HeaderMenuSeam | undefined,
): PaneHeaderMenu | null {
  if (cwd === undefined && seam === undefined) return null;
  return {
    copyPath: cwd ?? null,
    closeEntries: seam?.closeEntries ?? unwiredCloseEntries(),
    onCloseEntry: seam?.onCloseEntry ?? null,
  };
}

export function terminalHeaderMenu(
  cwd: string | undefined,
  onCloseTab: (() => void) | undefined,
  seam: HeaderMenuSeam | undefined,
): PaneHeaderMenu | null {
  if (cwd === undefined && seam === undefined && onCloseTab === undefined) return null;
  if (seam !== undefined) {
    return {
      copyPath: cwd ?? null,
      closeEntries: seam.closeEntries,
      onCloseEntry: seam.onCloseEntry,
    };
  }
  if (onCloseTab === undefined) {
    return { copyPath: cwd ?? null, closeEntries: unwiredCloseEntries(), onCloseEntry: null };
  }
  return {
    copyPath: cwd ?? null,
    closeEntries: terminalCloseEntries(),
    onCloseEntry: (key) => {
      if (key === "close") onCloseTab();
    },
  };
}
