import { usableBranch } from "../changesStatusCache";
import { displayPath } from "../../../lib/displayPath";
import type { TabMenuEntry } from "./tabCloseMenu";
import { toolTabSubject, type StripTab } from "./toolTabs";

export type TabCopyAction = "copy-session-id" | "copy-path" | "copy-address" | "copy-branch-name";

export function isTabCopyAction(key: TabMenuEntry["key"]): key is TabCopyAction {
  return (
    key === "copy-session-id" ||
    key === "copy-path" ||
    key === "copy-address" ||
    key === "copy-branch-name"
  );
}

/** Where a browser tab's page is, or null for every other tab and for a
 * page the strip has no address for. The id is never the answer: it is an
 * opaque handle, not something a person can paste anywhere. */
function browserAddress(
  tab: StripTab,
  resolve: ((browserId: string) => string | null) | undefined,
): string | null {
  if (tab.type !== "tool" || tab.tool.kind !== "browser" || resolve === undefined) return null;
  return resolve(tab.tool.browserId);
}

export function tabCopyValue(
  tab: StripTab,
  key: TabCopyAction,
  branch?: string | null,
  resolveBrowserAddress?: (browserId: string) => string | null,
): string | null {
  if (key === "copy-session-id") return tab.type === "session" ? tab.session.id : null;
  if (key === "copy-branch-name") return tab.type === "session" ? usableBranch(branch) : null;
  if (key === "copy-address") return browserAddress(tab, resolveBrowserAddress);
  // A page has no path to copy, and its id is not one.
  if (tab.type === "tool" && tab.tool.kind === "browser") return null;
  const path = tab.type === "session" ? tab.session.cwd : toolTabSubject(tab.tool);
  return displayPath(path ?? "") || null;
}

export function buildTabCopyEntries(
  tab: StripTab,
  branch?: string | null,
  resolveBrowserAddress?: (browserId: string) => string | null,
): TabMenuEntry[] {
  const entries: TabMenuEntry[] = [];
  const address = browserAddress(tab, resolveBrowserAddress);
  if (address !== null) {
    entries.push({ key: "copy-address", label: "Copy address", disabled: false });
  }
  if (address === null && tabCopyValue(tab, "copy-path", branch, resolveBrowserAddress) !== null) {
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
