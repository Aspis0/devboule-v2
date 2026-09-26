// Why: the tab context menu as data — Paseo's four close entries in Paseo's
// order (pinned by workspace-tab-menu.test.ts:47-56), then our Delete after
// a separator in the destructive tone, and when each entry has nothing to
// act on — so the menu only renders rows.

import type { TabCloseAction } from "./bulkCloseSessions";

export interface TabMenuEntry {
  key: TabCloseAction | "close-selection" | "delete";
  label: string;
  disabled: boolean;
  /** Rendered after a separator, in the destructive tone: it destroys. */
  destructive?: boolean;
}

export function buildTabCloseEntries(index: number, tabCount: number): TabMenuEntry[] {
  return [
    { key: "left", label: "Close to the left", disabled: index === 0 },
    { key: "right", label: "Close to the right", disabled: index === tabCount - 1 },
    { key: "others", label: "Close other tabs", disabled: tabCount <= 1 },
    { key: "close", label: "Close", disabled: false },
    { key: "delete", label: "Delete", disabled: false, destructive: true },
  ];
}

export function buildSelectionCloseEntry(selectionSize: number): TabMenuEntry {
  return {
    key: "close-selection",
    label: selectionSize === 1 ? "Close" : `Close ${selectionSize} tabs`,
    disabled: false,
  };
}
