import type { WorkspaceView } from "../workspaceProjects";
import { compactAge } from "./compactAge";
import type { WorkspaceStat } from "./useWorkspaceStats";

/** The row's state dot, in the tab chips' vocabulary. */
type WorkspaceStateDot = NonNullable<WorkspaceView["stateDot"]>;

/** The dot's names, for a row whose state is all the fact there is. */
const DOT_LABELS: Record<WorkspaceStateDot, string> = {
  pulse: "running",
  attention: "needs attention",
  unattended: "running unattended",
};

export interface RowFact {
  /** The dot drawn beside the label; null when the fact is not an agent state. */
  dot: WorkspaceStateDot | null;
  label: string;
}

/**
 * The one fact the row prints right of the name: what the workspace is doing,
 * else when it last spoke, else what is uncommitted — and nothing when it has
 * none of the three, so a quiet row prints its name alone.
 */
export function rowFact(workspace: WorkspaceView, stat: WorkspaceStat | undefined): RowFact | null {
  const { working, waiting } = workspace.agents;
  if (waiting > 0) return { dot: workspace.stateDot, label: `${waiting} waiting` };
  if (working > 0) return { dot: workspace.stateDot, label: `${working} working` };
  if (workspace.stateDot !== null) {
    return { dot: workspace.stateDot, label: DOT_LABELS[workspace.stateDot] };
  }
  const age = compactAge(workspace.elapsedMs);
  if (age !== null) return { dot: null, label: age };
  if (stat !== undefined && stat.additions + stat.deletions > 0) {
    return { dot: null, label: `+${stat.additions} −${stat.deletions}` };
  }
  return null;
}
