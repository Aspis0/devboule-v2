// The rail's agents as a React value. A roster push that changes nothing the rail
// reads hands back the same sessions and the same rows, so the tree below keeps
// its identity and the rows are not derived again.
import { useMemo, useState } from "react";
import type { Session } from "../../../types/ipc";
import { isAgentKind } from "../../../types/ipc";
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { localWorkspaceKey } from "../hosts/hostIdentity";
import { railAgentsOf, type RailAgent, type RailAgents } from "./railAgents";

function agentsOfWorkspace(sessions: readonly Session[], key: WorkspaceKey | null): Session[] {
  if (key === null) return [];
  return sessions.filter(
    (session) =>
      isAgentKind(session.kind) &&
      session.workspaceId !== null &&
      localWorkspaceKey(session.workspaceId) === key,
  );
}

/** Every field the rail reads from a session, compared as values. */
function sameSession(a: Session, b: Session): boolean {
  return (
    a.id === b.id &&
    a.kind === b.kind &&
    a.provider === b.provider &&
    a.workspaceId === b.workspaceId &&
    a.title === b.title &&
    a.displayName === b.displayName &&
    a.createdBy === b.createdBy &&
    a.activity === b.activity &&
    JSON.stringify(a.state) === JSON.stringify(b.state) &&
    JSON.stringify(a.attention) === JSON.stringify(b.attention)
  );
}

function sameSessions(a: readonly Session[], b: readonly Session[]): boolean {
  return (
    a.length === b.length &&
    a.every((session, index) => {
      const other = b[index];
      return other !== undefined && sameSession(session, other);
    })
  );
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
    a.hasChildren === b.hasChildren &&
    a.generation === b.generation &&
    a.archivable === b.archivable
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

/** The workspace's agent sessions, the same array while none of them changed. */
export function useRailAgentSessions(
  sessions: readonly Session[],
  key: WorkspaceKey | null,
): readonly Session[] {
  const fresh = useMemo(() => agentsOfWorkspace(sessions, key), [sessions, key]);
  const [stable, setStable] = useState(fresh);
  const current = sameSessions(stable, fresh) ? stable : fresh;
  if (current !== stable) setStable(current);
  return current;
}

/** The rail's rows for the selected workspace, the same object while they are unchanged. */
export function useRailAgents(
  sessions: readonly Session[],
  key: WorkspaceKey | null,
  activeId: string | null,
): RailAgents {
  const scoped = useRailAgentSessions(sessions, key);
  const fresh = useMemo(() => railAgentsOf(scoped, key, activeId), [scoped, key, activeId]);
  const [stable, setStable] = useState(fresh);
  const current = sameRailAgents(stable, fresh) ? stable : fresh;
  if (current !== stable) setStable(current);
  return current;
}
