import type { WorkspaceView } from "../workspaceProjects";

/**
 * What the status dot of a workspace row says. Activity only: an approval the
 * person owes, a turn in progress, or nothing going on. The permission mode a
 * session runs under is never a status, so it never reaches the row.
 */
export type RowStatusTone = "running" | "attention" | "idle";

export interface RowStatus {
  tone: RowStatusTone;
  /** The dot's name for a screen reader, and for the row's accessible name. */
  label: string;
}

const LABELS: Record<RowStatusTone, string> = {
  running: "running",
  attention: "needs you",
  idle: "idle",
};

/**
 * The row's status dot, in the slot the letter avatar held. An approval owed
 * outranks a running turn: a row that waits on the person says so first.
 */
export function rowStatus(workspace: WorkspaceView): RowStatus {
  const tone: RowStatusTone =
    workspace.stateDot === "attention"
      ? "attention"
      : workspace.stateDot === "pulse"
        ? "running"
        : "idle";
  return { tone, label: LABELS[tone] };
}
