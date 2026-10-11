import type { Session, SessionKind } from "../../../types/ipc";
import { isAgentKind } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { providerName } from "../providerNames";
import { sessionNeedsApproval } from "../sessionAttention";
import { childRow } from "../subagentRows";
import { sessionTitle } from "../workspaceSessions";
import type { WorkspaceAgents } from "../workspaceProjects";

/** Rows the rail lists under the selected workspace; the rest wait behind `+N more in History`. */
export const RAIL_AGENT_CAP = 8;

export type RailAgentState = "running" | "waiting" | "finished" | "failed" | "stopped" | "idle";

export interface RailAgent {
  id: string;
  kind: SessionKind;
  /** The provider's printed name; null for an id the app has no name for. */
  provider: string | null;
  title: string;
  state: RailAgentState;
  /** Nested one step in: the agent was created by another listed agent. */
  child: boolean;
  /** The creator's title for a child; null for a root. */
  parentTitle: string | null;
}

export interface RailAgents {
  rows: readonly RailAgent[];
  /** Rows past the cap, which History lists. */
  hidden: number;
}

const NO_AGENTS: RailAgents = { rows: [], hidden: 0 };

// A child's own state comes first: a waiting or running child reads before a
// failed one, which reads before a settled one.
const CHILD_ORDER: Readonly<Record<RailAgentState, number>> = {
  waiting: 0,
  running: 1,
  failed: 2,
  stopped: 3,
  finished: 3,
  idle: 3,
};

function stateOf(session: Session, child: boolean): RailAgentState {
  if (sessionNeedsApproval(session)) return "waiting";
  if (session.activity === "working") return "running";
  const status = childRow(session).status;
  if (status === "failed" || status === "stopped") return status;
  // A root that is still open and quiet is idle; only a child, or a root that
  // has ended, says it finished.
  if (status === "finished" && (child || session.state.type === "ended")) return "finished";
  return "idle";
}

/**
 * The agents of one workspace as the rail lists them: each root in roster
 * order, its children after it in state order, and their children after them.
 * Terminals are not agents and are never listed.
 */
export function railAgentsOf(sessions: readonly Session[], key: WorkspaceKey | null): RailAgents {
  if (key === null) return NO_AGENTS;
  const agents = sessions.filter(
    (session) =>
      isAgentKind(session.kind) &&
      session.workspaceId !== null &&
      localWorkspaceKey(session.workspaceId) === key,
  );
  const byId = new Map(agents.map((session) => [session.id, session]));
  // An agent whose creator is not listed here is a root, so it is never lost.
  const parentOf = (session: Session): string | undefined => {
    const parent = session.createdBy;
    return parent !== undefined && parent !== session.id && byId.has(parent) ? parent : undefined;
  };
  const childrenOf = new Map<string, Session[]>();
  for (const session of agents) {
    const parent = parentOf(session);
    if (parent === undefined) continue;
    const list = childrenOf.get(parent);
    if (list === undefined) childrenOf.set(parent, [session]);
    else list.push(session);
  }

  const rows: RailAgent[] = [];
  const visit = (session: Session, parent: Session | null) => {
    const child = parent !== null;
    rows.push({
      id: session.id,
      kind: session.kind,
      provider: providerName(session.provider ?? session.kind),
      title: sessionTitle(session),
      state: stateOf(session, child),
      child,
      parentTitle: parent === null ? null : sessionTitle(parent),
    });
    const children = (childrenOf.get(session.id) ?? [])
      .map((kid) => ({ kid, order: CHILD_ORDER[stateOf(kid, true)] }))
      .sort((a, b) => a.order - b.order);
    for (const { kid } of children) visit(kid, session);
  };
  for (const session of agents) {
    if (parentOf(session) === undefined) visit(session, null);
  }

  return {
    rows: rows.slice(0, RAIL_AGENT_CAP),
    hidden: Math.max(0, rows.length - RAIL_AGENT_CAP),
  };
}

/**
 * The count line a workspace row not in front shows: a waiting agent outranks
 * the working ones. Null when nobody works or waits.
 */
export function agentCountLine(agents: WorkspaceAgents): { text: string; waiting: boolean } | null {
  if (agents.waiting > 0) return { text: `${agents.waiting} needs you`, waiting: true };
  if (agents.working > 0) return { text: `${agents.working} working`, waiting: false };
  return null;
}
