import type { WorkspaceGitDiffLine } from "../../types/ipc";

// The daemon strips the `+`/`-`/space marker, so both diff surfaces draw it here;
// the non-breaking space keeps context and header lines aligned under the content column.
export const DIFF_LINE_MARKER: Record<WorkspaceGitDiffLine["kind"], string> = {
  add: "+",
  remove: "\u2212",
  context: "\u00A0",
  header: "\u00A0",
};

export const DIFF_ROW_WORD: Record<WorkspaceGitDiffLine["kind"], string | null> = {
  add: "added",
  remove: "removed",
  context: null,
  header: null,
};
