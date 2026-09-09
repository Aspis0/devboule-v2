// MOCK DATA ONLY — this is a UI view model over the future IPC entities. The
// shared entity fields stay aligned, while presentation fields remain local to
// the Workspace surface.

export const MOCK_DIFF_LINES = [
  { line: "18", text: "impl IndexWriter {", kind: "context" as const },
  { line: "−", text: "  pub fn flush(&mut self) -> Result<()> {", kind: "removed" as const },
  { line: "+", text: "  pub async fn flush(&mut self) -> Result<usize> {", kind: "added" as const },
  { line: "+", text: "    let batch = self.pending.drain(..);", kind: "added" as const },
  { line: "+", text: "    self.table.add(batch).await?;", kind: "added" as const },
  { line: "24", text: "  }", kind: "context" as const },
];

export const MOCK_SHIP_STEPS = ["Worktree", "Preview", "Review", "Commit", "PR", "Merge"];
