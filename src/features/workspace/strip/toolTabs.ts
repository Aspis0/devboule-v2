// Why: a Diff or File tab is frontend state only — the daemon knows
// sessions, never these. This owns the tab's shape, its id namespace (a
// `tool:` prefix session ids cannot collide with), the strip order (tool
// tabs after sessions) and the one successor rule every close path shares,
// so a tool close lands focus exactly where a session close would.

import type { Session } from "../../../types/ipc";
import { parseWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";

export type ToolTabKind = "diff" | "file";

export interface ToolTab {
  id: string;
  kind: ToolTabKind;
  /** What the strip filters and prunes on: the workspace as the UI names it. */
  workspaceKey: WorkspaceKey;
  /** Workspace-relative, `/`-joined — the trees' own spelling. */
  path: string;
}

export type StripTab =
  | { type: "session"; id: string; session: Session }
  | { type: "tool"; id: string; tool: ToolTab };

/** Session ids embed a per-daemon-process nonce; the `tool:` prefix keeps
 * this namespace disjoint from them by construction. Each part is encoded
 * before joining, so a `:` inside a workspace id or path cannot collide
 * with the separators — the id is compared, never parsed.
 *
 * The workspace is named by id alone: a tab id that carried its host would
 * change the string of every Diff and File tab in a strip, remounting every
 * chip for no gain. Two hosts can therefore mint the same id, and the id
 * alone is not what separates their tabs — the strip never shows both. */
export function toolTabId(kind: ToolTabKind, workspaceId: string, path: string): string {
  return `tool:${kind}:${encodeURIComponent(workspaceId)}:${encodeURIComponent(path)}`;
}

export function isToolTabId(id: string): boolean {
  return id.startsWith("tool:");
}

export function makeToolTab(kind: ToolTabKind, workspaceKey: WorkspaceKey, path: string): ToolTab {
  return {
    id: toolTabId(kind, parseWorkspaceKey(workspaceKey).workspaceId, path),
    kind,
    workspaceKey,
    path,
  };
}

/** Opening twice focuses; it never duplicates. */
export function openToolTabs(tabs: ToolTab[], tab: ToolTab): ToolTab[] {
  if (tabs.some((current) => current.id === tab.id)) return tabs;
  return [...tabs, tab];
}

export function toolTabLabel(path: string): string {
  const base = path.split("/").pop() ?? "";
  return base === "" ? path : base;
}

export function toolTabDirectory(path: string): string | null {
  const slash = path.lastIndexOf("/");
  return slash > 0 && slash < path.length - 1 ? path.slice(0, slash) : null;
}

export function composeStripTabs(
  sessions: readonly Session[],
  tools: readonly ToolTab[],
): StripTab[] {
  return [
    ...sessions.map((session): StripTab => ({ type: "session", id: session.id, session })),
    ...tools.map((tool): StripTab => ({ type: "tool", id: tool.id, tool })),
  ];
}

/** Where focus and selection land when a close took the active tab: the
 * nearest survivor to the RIGHT of the closed active tab, else the nearest
 * to the left; with none left, no active tab. Shared by the session flow
 * and the tool close so both land identically. */
export function successorOf(
  orderedIds: readonly string[],
  closedIds: readonly string[],
  activeId: string,
): string | null {
  const closed = new Set(closedIds);
  if (!closed.has(activeId)) return activeId;
  const activeIndex = orderedIds.findIndex((id) => id === activeId);
  // Unreachable through the app, but a stale id still lands on the first
  // survivor instead of deselecting.
  if (activeIndex === -1) return orderedIds.find((id) => !closed.has(id)) ?? null;
  const right = orderedIds.slice(activeIndex + 1).find((id) => !closed.has(id));
  if (right !== undefined) return right;
  const left = [...orderedIds.slice(0, activeIndex)].reverse().find((id) => !closed.has(id));
  return left ?? null;
}

export function pruneToolTabsForWorkspaces(
  tabs: ToolTab[],
  knownWorkspaceKeys: ReadonlySet<WorkspaceKey>,
): ToolTab[] {
  if (tabs.every((tab) => knownWorkspaceKeys.has(tab.workspaceKey))) return tabs;
  return tabs.filter((tab) => knownWorkspaceKeys.has(tab.workspaceKey));
}
