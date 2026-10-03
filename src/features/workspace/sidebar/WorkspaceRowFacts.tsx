import type { WorkspaceAgents, WorkspaceView } from "../workspaceProjects";
import type { WorkspaceStat } from "./useWorkspaceStats";

/** The row's state dot, in the tab chips' vocabulary. */
type WorkspaceStateDot = NonNullable<WorkspaceView["stateDot"]>;

/** The dot's names, spoken by the row's own accessible label. */
export const DOT_LABELS: Record<WorkspaceStateDot, string> = {
  pulse: "running",
  attention: "needs attention",
  unattended: "running unattended",
};

export interface WorkspaceRowFactsProps {
  /** The branch the workspace's last status read reported; undefined when the
   * folder is not a repository, or the read failed. */
  branch: string | undefined;
  /** The diff against the branch base, when the read reported one. */
  stat: WorkspaceStat | undefined;
  agents: WorkspaceAgents;
  stateDot: WorkspaceStateDot | null;
}

/**
 * The row's second line: what the sidebar already knows about this workspace.
 * Nothing here is invented for the row — every part is a read the sidebar was
 * making anyway — and a part with no fact is left out rather than blanked.
 */
export function WorkspaceRowFacts({ branch, stat, agents, stateDot }: WorkspaceRowFactsProps) {
  const hasAgents = stateDot !== null || agents.working > 0 || agents.waiting > 0;
  if (branch === undefined && stat === undefined && !hasAgents) return null;
  return (
    <span className="workspace-row-facts">
      {branch === undefined ? null : <span className="workspace-row-branch">{branch}</span>}
      {stat === undefined ? null : (
        <span className="sidebar-row-stats">
          <span className="sidebar-stat-add">+{stat.additions}</span>{" "}
          <span className="sidebar-stat-del">−{stat.deletions}</span>
        </span>
      )}
      {hasAgents ? (
        <span className="sidebar-row-agents">
          {stateDot === null ? null : (
            <span
              role="img"
              aria-label={DOT_LABELS[stateDot]}
              className={`sidebar-row-dot sidebar-row-dot-${stateDot}${
                stateDot === "pulse" ? " dot-pulse" : ""
              }`}
            />
          )}
          {agents.working === 0 ? null : (
            <span className="sidebar-row-working">{agents.working} working</span>
          )}
          {agents.waiting === 0 ? null : (
            <span className="sidebar-row-waiting">{agents.waiting} waiting</span>
          )}
        </span>
      ) : null}
    </span>
  );
}
