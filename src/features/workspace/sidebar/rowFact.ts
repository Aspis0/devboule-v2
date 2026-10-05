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
  /** The dot drawn beside the fact; null when the fact is not an agent state. */
  dot: WorkspaceStateDot | null;
  /** The pieces in print order. An approval owed comes first and is the only
   *  one that carries the attention tone — the count beside it never buries it. */
  parts: readonly { text: string; attention: boolean }[];
}

/**
 * The one fact the row prints right of the name: what the workspace is doing,
 * else when it last spoke, else what is uncommitted — and nothing when it has
 * none of the three, so a quiet row prints its name alone.
 */
export function rowFact(workspace: WorkspaceView, stat: WorkspaceStat | undefined): RowFact | null {
  const { working, waiting } = workspace.agents;
  const parts: { text: string; attention: boolean }[] = [];
  if (waiting > 0) parts.push({ text: `${waiting} waiting`, attention: true });
  if (working > 0) parts.push({ text: `${working} working`, attention: false });
  if (parts.length === 0) {
    if (workspace.stateDot !== null) {
      parts.push({ text: DOT_LABELS[workspace.stateDot], attention: false });
    } else {
      const age = compactAge(workspace.elapsedMs);
      if (age !== null) parts.push({ text: age, attention: false });
      else if (stat !== undefined && stat.additions + stat.deletions > 0) {
        parts.push({ text: `+${stat.additions} −${stat.deletions}`, attention: false });
      }
    }
  }
  if (parts.length === 0) return null;
  return { dot: workspace.stateDot, parts };
}
