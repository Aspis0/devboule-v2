import { usableBranch } from "../changesStatusCache";
import { displayPath } from "../../../lib/displayPath";
import type { TabMenuEntry } from "./tabCloseMenu";
import { toolTabSubject, type StripTab } from "./toolTabs";

export type TabCopyAction = "copy-session-id" | "copy-path" | "copy-branch-name";

export function isTabCopyAction(key: TabMenuEntry["key"]): key is TabCopyAction {
  return key === "copy-session-id" || key === "copy-path" || key === "copy-branch-name";
}

export function tabCopyValue(
  tab: StripTab,
  key: TabCopyAction,
  branch?: string | null,
): string | null {
  if (key === "copy-session-id") return tab.type === "session" ? tab.session.id : null;
  if (key === "copy-branch-name") return tab.type === "session" ? usableBranch(branch) : null;
  const path = tab.type === "session" ? tab.session.cwd : toolTabSubject(tab.tool);
  return displayPath(path ?? "") || null;
}

export function buildTabCopyEntries(tab: StripTab, branch?: string | null): TabMenuEntry[] {
  const entries: TabMenuEntry[] = [];
  if (tab.type === "session") {
    entries.push({ key: "copy-session-id", label: "Copy session ID", disabled: false });
  }
  if (tabCopyValue(tab, "copy-path") !== null) {
    entries.push({
      key: "copy-path",
      label: tab.type === "tool" ? "Copy relative path" : "Copy path",
      disabled: false,
    });
  }
  if (tabCopyValue(tab, "copy-branch-name", branch) !== null) {
    entries.push({ key: "copy-branch-name", label: "Copy branch name", disabled: false });
  }
  const last = entries.at(-1);
  if (last !== undefined) last.separatorAfter = true;
  return entries;
}
