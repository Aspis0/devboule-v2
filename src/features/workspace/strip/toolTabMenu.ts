// Why: tool tabs have no daemon session to rename or delete.

import { buildToolTabCloseEntries, type TabMenuEntry } from "./tabCloseMenu";
import type { StripTab } from "./toolTabs";
import { buildTabCopyEntries } from "./tabCopyActions";

/** The actions for a tool anchor, or null when the anchor is not one. */
export function toolTabMenuEntries(
  tabs: readonly StripTab[],
  anchorId: string,
): TabMenuEntry[] | null {
  const index = tabs.findIndex((tab) => tab.id === anchorId);
  if (index === -1) return null;
  const tab = tabs[index];
  if (tab?.type !== "tool") return null;
  return [...buildTabCopyEntries(tab), ...buildToolTabCloseEntries(index, tabs.length)];
}
