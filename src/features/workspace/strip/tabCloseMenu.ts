// Why: the tab context menu as data — the four close entries in the order
// workspace-tab-menu.test.ts pins, Delete after a separator in the
// destructive tone, and when each entry has nothing to act on —
// so the menu only renders rows.

import type { TabCloseAction } from "./bulkCloseSessions";
import type { TabCopyAction } from "./tabCopyActions";

export interface TabMenuEntry {
  key:
    | TabCloseAction
    | "close-selection"
    | "archive"
    | "delete"
    | "rename"
    | "split-down"
    | "move-up-pane"
    | TabCopyAction;
  label: string;
  disabled: boolean;
  /** Rendered after a separator, in the destructive tone: it destroys. */
  destructive?: boolean;
  /** Copies and rename act on a tab; the separator keeps them apart from closing tabs. */
  separatorAfter?: boolean;
}

/** Archive sits after Close and before Delete; only an agent tab has a
 * session to archive, so the entry is offered when the caller says so. */
export function buildTabCloseEntries(
  index: number,
  tabCount: number,
  archivable = false,
): TabMenuEntry[] {
  return [
    { key: "left", label: "Close to the left", disabled: index === 0 },
    { key: "right", label: "Close to the right", disabled: index === tabCount - 1 },
    { key: "others", label: "Close other tabs", disabled: tabCount <= 1 },
    { key: "close", label: "Close", disabled: false },
    ...(archivable ? [{ key: "archive" as const, label: "Archive", disabled: false }] : []),
    { key: "delete", label: "Delete", disabled: false, destructive: true },
  ];
}

/** A tool tab's menu: the close entries only. No Rename (the rename half
 * resolves the anchor in the session roster) and no Delete (it would
 * destroy a session id the daemon never knew). */
export function buildToolTabCloseEntries(index: number, tabCount: number): TabMenuEntry[] {
  return [
    { key: "left", label: "Close to the left", disabled: index === 0 },
    { key: "right", label: "Close to the right", disabled: index === tabCount - 1 },
    { key: "others", label: "Close other tabs", disabled: tabCount <= 1 },
    { key: "close", label: "Close", disabled: false },
  ];
}

export function buildSelectionCloseEntry(selectionSize: number): TabMenuEntry {
  return {
    key: "close-selection",
    label: selectionSize === 1 ? "Close" : `Close ${selectionSize} tabs`,
    disabled: false,
  };
}
