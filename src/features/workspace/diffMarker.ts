import type { WorkspaceGitDiffLine } from "../../types/ipc";

// The daemon strips the `+`/`-`/space marker, so both diff surfaces draw it here.
export const DIFF_LINE_MARKER: Record<WorkspaceGitDiffLine["kind"], string> = {
  add: "+",
  remove: "\u2212",
  context: "\u00A0",
  header: "\u00A0",
};
