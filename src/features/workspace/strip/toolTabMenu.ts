// Why: tool tabs have no daemon session to rename or delete.

import { buildToolTabCloseEntries, type TabMenuEntry } from "./tabCloseMenu";
import type { StripTab, ToolTab } from "./toolTabs";
import { buildTabCopyEntries } from "./tabCopyActions";

/** The actions for a tool anchor, or null when the anchor is not one. */
export function toolTabMenuEntries(
  tabs: readonly StripTab[],
  anchorId: string,
  resolveBrowserAddress?: (browserId: string) => string | null,
  paneActs?: PaneActs,
): TabMenuEntry[] | null {
  const index = tabs.findIndex((tab) => tab.id === anchorId);
  if (index === -1) return null;
  const tab = tabs[index];
  if (tab?.type !== "tool") return null;
  return [
    ...buildTabCopyEntries(tab, null, resolveBrowserAddress),
    ...paneEntries(tab.tool, paneActs),
    ...buildToolTabCloseEntries(index, tabs.length),
  ];
}

/** What the pane can still do with this tab, told by the workspace because only
 * it knows which pane the tab is in. */
export interface PaneActs {
  /** The tab is in the pane below, so it can come back up and the split end. */
  isBelow: boolean;
}

/** The keyboard's road into the split, next to the drag: the same two acts the
 * drop zones make, named on the tab they act on. Only a browser tab carries one
 * — the pane below holds a page, not a conversation. */
function paneEntries(tab: ToolTab, acts: PaneActs | undefined): TabMenuEntry[] {
  if (tab.kind !== "browser") return [];
  const isBelow = acts?.isBelow === true;
  return isBelow
    ? [{ key: "move-up-pane", label: "Move out of the pane below", disabled: false }]
    : [{ key: "split-down", label: "Move to the pane below", disabled: false }];
}
