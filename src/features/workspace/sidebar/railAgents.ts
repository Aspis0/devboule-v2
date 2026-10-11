import type { Session, SessionKind } from "../../../types/ipc";
import { isAgentKind } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { providerName } from "../providerNames";
import { sessionNeedsApproval } from "../sessionAttention";
import { childRow, isArchivable } from "../subagentRows";
import type { WorkspaceAgents } from "../workspaceProjects";
import { sessionTitle } from "../workspaceSessions";

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
  parentId: string | null;
  /** The creator's title for a child; null for a root. */
  parentTitle: string | null;
  /** Another shown agent sits directly under this one. */
  hasChildren: boolean;
  /** The generation the row was read at: an archive closes only that one. */
  generation: number | null;
  /** A child the roster can close now, as the pane list offers it. */
  archivable: boolean;
}

export interface RailAgents {
  rows: readonly RailAgent[];
  /** Agents past the cap, which History lists. */
  hidden: number;
}

type TreeNode = Omit<RailAgent, "hasChildren">;

interface Tree {
  nodes: TreeNode[];
  /** The ids of agents whose process has ended, which the cap cuts first. */
  settled: ReadonlySet<string>;
}

const NO_AGENTS: RailAgents = { rows: [], hidden: 0 };

// A waiting child reads before a running one, a running one before a failed one,
// and the rest in roster order.
const CHILD_ORDER: Readonly<Record<RailAgentState, number>> = {
  waiting: 0,
  running: 1,
  failed: 2,
  finished: 3,
  stopped: 3,
  idle: 3,
};

function isSettled(session: Session): boolean {
  return session.state.type === "ended" || session.state.type === "recovered";
}

function stateOf(session: Session, child: boolean): RailAgentState {
  if (sessionNeedsApproval(session)) return "waiting";
  const status = childRow(session).status;
  if (status === "running") return "running";
  if (status === "failed" || status === "stopped") return status;
  return status === "finished" && child ? "finished" : "idle";
}

/**
 * Every listed agent in tree order: each root, then its children, then theirs.
 * A listed agent is a root when its process is up, or a child of an agent this
 * workspace holds, whichever state that agent is in. A child whose creator has
 * ended keeps its place as a child, named after that creator.
 */
function treeOf(sessions: readonly Session[], key: WorkspaceKey): Tree {
  const ofWorkspace = sessions.filter(
    (session) =>
      isAgentKind(session.kind) &&
      session.workspaceId !== null &&
      localWorkspaceKey(session.workspaceId) === key,
  );
  const byId = new Map(ofWorkspace.map((session) => [session.id, session]));
  const parentOf = (session: Session): Session | undefined => {
    const parent = session.createdBy;
    return parent === undefined || parent === session.id ? undefined : byId.get(parent);
  };
  const listed = ofWorkspace.filter(
    (session) =>
      parentOf(session) !== undefined ||
      session.state.type === "live" ||
      session.state.type === "silent",
  );
  const listedIds = new Set(listed.map((session) => session.id));
  const childrenOf = new Map<string, Session[]>();
  for (const session of listed) {
    const parent = parentOf(session);
    if (parent === undefined || !listedIds.has(parent.id)) continue;
    const list = childrenOf.get(parent.id);
    if (list === undefined) childrenOf.set(parent.id, [session]);
    else list.push(session);
  }

  const nodes: TreeNode[] = [];
  const visit = (session: Session) => {
    const parent = parentOf(session);
    const child = parent !== undefined;
    nodes.push({
      id: session.id,
      kind: session.kind,
      provider: providerName(session.provider ?? session.kind),
      title: sessionTitle(session),
      state: stateOf(session, child),
      child,
      parentId: parent?.id ?? null,
      parentTitle: parent === undefined ? null : sessionTitle(parent),
      generation: childRow(session).generation,
      archivable: child && isArchivable(childRow(session)),
    });
    const children = [...(childrenOf.get(session.id) ?? [])].sort(
      (a, b) => CHILD_ORDER[stateOf(a, true)] - CHILD_ORDER[stateOf(b, true)],
    );
    for (const kid of children) visit(kid);
  };
  for (const session of listed) {
    const parent = parentOf(session);
    if (parent === undefined || !listedIds.has(parent.id)) visit(session);
  }
  const settled = new Set(listed.filter(isSettled).map((session) => session.id));
  return { nodes, settled };
}

/**
 * The agents of one workspace as the rail lists them. Past the cap, what shows
 * is chosen by need: a waiting, running or front agent is never cut, then the
 * live rows, then the settled children. A creator is kept with the rows it holds.
 */
export function railAgentsOf(
  sessions: readonly Session[],
  key: WorkspaceKey | null,
  activeId: string | null,
): RailAgents {
  if (key === null) return NO_AGENTS;
  const { nodes: tree, settled } = treeOf(sessions, key);
  const byId = new Map(tree.map((node) => [node.id, node]));
  const kept = new Set<string>();
  const keep = (id: string) => {
    let current: string | null = id;
    while (current !== null && byId.has(current) && !kept.has(current)) {
      kept.add(current);
      current = byId.get(current)?.parentId ?? null;
    }
  };
  for (const node of tree) {
    if (node.state === "waiting" || node.state === "running" || node.id === activeId) {
      keep(node.id);
    }
  }
  const fill = [
    ...tree.filter((node) => !settled.has(node.id)),
    ...tree.filter((node) => settled.has(node.id)),
  ];
  for (const node of fill) {
    if (kept.size >= RAIL_AGENT_CAP) break;
    keep(node.id);
  }
  const shown = tree.filter((node) => kept.has(node.id));
  const rows = shown.map((node) => ({
    ...node,
    hasChildren: shown.some((other) => other.parentId === node.id),
  }));
  return { rows, hidden: tree.length - shown.length };
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
