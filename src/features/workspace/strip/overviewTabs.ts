import type { Session } from "../../../types/ipc";
import { sessionNeedsApproval } from "../sessionAttention";
import { orderOverviewSessions } from "./sessionOverview";
import type { StripTab, ToolTab } from "./toolTabs";

export type OverviewRow =
  | { kind: "session"; id: string; session: Session; open: boolean }
  | { kind: "tool"; id: string; tool: ToolTab };

export interface OverviewGroup {
  key: "attention" | "tabs" | "sessions";
  label: string;
  rows: OverviewRow[];
}

export function composeOverviewGroups(
  tabs: readonly StripTab[],
  sessions: readonly Session[],
  stripOrder: readonly string[],
): OverviewGroup[] {
  const tabIds = new Set(tabs.map((tab) => tab.id));
  const byId = new Map(sessions.map((session) => [session.id, session]));
  const unopened = orderOverviewSessions(sessions, stripOrder).filter(
    (session) => !tabIds.has(session.id),
  );
  const attention: OverviewRow[] = unopened
    .filter((session) => sessionNeedsApproval(session))
    .map((session): OverviewRow => ({ kind: "session", id: session.id, session, open: false }));
  const tabRows: OverviewRow[] = tabs.map((tab) =>
    tab.type === "tool"
      ? { kind: "tool", id: tab.id, tool: tab.tool }
      : {
          kind: "session",
          id: tab.id,
          session: byId.get(tab.id) ?? tab.session,
          open: true as const,
        },
  );
  const rest: OverviewRow[] = unopened
    .filter((session) => !sessionNeedsApproval(session))
    .map((session): OverviewRow => ({ kind: "session", id: session.id, session, open: false }));
  const groups: OverviewGroup[] = [];
  if (attention.length > 0)
    groups.push({ key: "attention", label: "Needs your approval", rows: attention });
  if (tabRows.length > 0) groups.push({ key: "tabs", label: "Open tabs", rows: tabRows });
  if (rest.length > 0) groups.push({ key: "sessions", label: "Other sessions", rows: rest });
  return groups;
}
