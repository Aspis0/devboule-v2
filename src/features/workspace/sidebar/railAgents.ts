import { useMemo, useState } from "react";
import type { Session, SessionKind } from "../../../types/ipc";
import { isAgentKind } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { providerName } from "../providerNames";
import { sessionNeedsApproval } from "../sessionAttention";
import { childRow } from "../subagentRows";
import type { WorkspaceAgents } from "../workspaceProjects";
import { sessionTitle } from "../workspaceSessions";

/** Rows the rail lists under the selected workspace; the rest wait behind `+N more in History`. */
export const RAIL_AGENT_CAP = 8;

export type RailAgentState = "running" | "waiting" | "finished" | "idle";

export interface RailAgent {
  id: string;
  kind: SessionKind;
  /** The provider's printed name; null for an id the app has no name for. */
  provider: string | null;
  title: string;
  state: RailAgentState;
  /** Nested one step in: the agent was created by another listed agent. */
  child: boolean;
  parentId: string | null;
  /** The creator's title for a child; null for a root. */
  parentTitle: string | null;
  /** Another shown agent sits directly under this one. */
  hasChildren: boolean;
}

export interface RailAgents {
  rows: readonly RailAgent[];
  /** Agents past the cap, which History lists. */
  hidden: number;
}

type TreeNode = Omit<RailAgent, "hasChildren">;

const NO_AGENTS: RailAgents = { rows: [], hidden: 0 };

// A waiting child reads before a running one, and a running one before the rest.
const CHILD_ORDER: Readonly<Record<RailAgentState, number>> = {
  waiting: 0,
  running: 1,
  finished: 2,
  idle: 2,
};

/** Only a process that is up is listed: an ended one has no place on the rail. */
function isListed(session: Session): boolean {
  return session.state.type === "live" || session.state.type === "silent";
}

function stateOf(session: Session, child: boolean): RailAgentState {
  if (sessionNeedsApproval(session)) return "waiting";
  const status = childRow(session).status;
  if (status === "running") return "running";
  return status === "finished" && child ? "finished" : "idle";
}

/** Every listed agent in tree order: each root, then its children, then theirs. */
function treeOf(sessions: readonly Session[], key: WorkspaceKey): TreeNode[] {
  const agents = sessions.filter(
    (session) =>
      isAgentKind(session.kind) &&
      session.workspaceId !== null &&
      localWorkspaceKey(session.workspaceId) === key &&
      isListed(session),
  );
  const byId = new Map(agents.map((session) => [session.id, session]));
  // An agent whose creator is not listed here is a root, so it is never lost.
  const parentOf = (session: Session): Session | undefined => {
    const parent = session.createdBy;
    return parent === undefined || parent === session.id ? undefined : byId.get(parent);
  };
  const childrenOf = new Map<string, Session[]>();
  for (const session of agents) {
    const parent = parentOf(session);
    if (parent === undefined) continue;
    const list = childrenOf.get(parent.id);
    if (list === undefined) childrenOf.set(parent.id, [session]);
    else list.push(session);
  }

  const nodes: TreeNode[] = [];
  const visit = (session: Session, parent: Session | undefined) => {
    nodes.push({
      id: session.id,
      kind: session.kind,
      provider: providerName(session.provider ?? session.kind),
      title: sessionTitle(session),
      state: stateOf(session, parent !== undefined),
      child: parent !== undefined,
      parentId: parent?.id ?? null,
      parentTitle: parent === undefined ? null : sessionTitle(parent),
    });
    const children = [...(childrenOf.get(session.id) ?? [])].sort(
      (a, b) => CHILD_ORDER[stateOf(a, true)] - CHILD_ORDER[stateOf(b, true)],
    );
    for (const kid of children) visit(kid, session);
  };
  for (const session of agents) {
    if (parentOf(session) === undefined) visit(session, undefined);
  }
  return nodes;
}

/**
 * The agents of one workspace as the rail lists them. Past the cap, what shows
 * is chosen by need: a waiting, running or front agent is never cut, and the
 * creator of a kept child stays with it.
 */
export function railAgentsOf(
  sessions: readonly Session[],
  key: WorkspaceKey | null,
  activeId: string | null,
): RailAgents {
  if (key === null) return NO_AGENTS;
  const tree = treeOf(sessions, key);
  const byId = new Map(tree.map((node) => [node.id, node]));
  const kept = new Set<string>();
  for (const node of tree) {
    if (node.state === "waiting" || node.state === "running" || node.id === activeId) {
      kept.add(node.id);
    }
  }
  for (const id of [...kept]) {
    let parent = byId.get(id)?.parentId ?? null;
    while (parent !== null && !kept.has(parent)) {
      kept.add(parent);
      parent = byId.get(parent)?.parentId ?? null;
    }
  }
  // Tree order puts a creator ahead of its children, so a child is only added
  // once its creator already is.
  for (const node of tree) {
    if (kept.size >= RAIL_AGENT_CAP) break;
    kept.add(node.id);
  }
  const shown = tree.filter((node) => kept.has(node.id));
  const rows = shown.map((node) => ({
    ...node,
    hasChildren: shown.some((other) => other.parentId === node.id),
  }));
  return { rows, hidden: tree.length - shown.length };
}

function sameRow(a: RailAgent, b: RailAgent): boolean {
  return (
    a.id === b.id &&
    a.kind === b.kind &&
    a.provider === b.provider &&
    a.title === b.title &&
    a.state === b.state &&
    a.child === b.child &&
    a.parentId === b.parentId &&
    a.parentTitle === b.parentTitle &&
    a.hasChildren === b.hasChildren
  );
}

function sameRailAgents(a: RailAgents, b: RailAgents): boolean {
  return (
    a.hidden === b.hidden &&
    a.rows.length === b.rows.length &&
    a.rows.every((row, index) => {
      const other = b.rows[index];
      return other !== undefined && sameRow(row, other);
    })
  );
}

/**
 * The rail's agents for the selected workspace. A roster push that leaves them
 * unchanged hands back the same object, so the rail below does not repaint.
 */
export function useRailAgents(
  sessions: readonly Session[],
  key: WorkspaceKey | null,
  activeId: string | null,
): RailAgents {
  const fresh = useMemo(() => railAgentsOf(sessions, key, activeId), [sessions, key, activeId]);
  const [stable, setStable] = useState(fresh);
  const current = sameRailAgents(stable, fresh) ? stable : fresh;
  if (current !== stable) setStable(current);
  return current;
}

/**
 * The count line a workspace row not in front shows: a waiting agent outranks
 * the working ones. Null when nobody works or waits.
 */
export function agentCountLine(agents: WorkspaceAgents): { text: string; waiting: boolean } | null {
  if (agents.waiting > 0) {
    const verb = agents.waiting === 1 ? "needs" : "need";
    return { text: `${agents.waiting} ${verb} you`, waiting: true };
  }
  if (agents.working > 0) return { text: `${agents.working} working`, waiting: false };
  return null;
}
