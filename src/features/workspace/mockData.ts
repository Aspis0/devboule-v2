// MOCK DATA ONLY — this is a UI view model over the future IPC entities. The
// shared entity fields stay aligned, while presentation fields remain local to
// the Workspace surface.

export interface MockSurface {
  id: "changes" | "files" | "app" | "design" | "pr";
  name: string;
  meta: string;
  dotTone: "terracotta" | "silence" | "green" | "purple" | "ochre";
}

export const MOCK_SURFACES: MockSurface[] = [
  { id: "changes", name: "Changes", meta: "+118 −64", dotTone: "terracotta" },
  { id: "files", name: "Files", meta: "2 140", dotTone: "silence" },
  { id: "app", name: "Interactive app", meta: "localhost", dotTone: "green" },
  { id: "design", name: "Design", meta: "1 generation", dotTone: "purple" },
  { id: "pr", name: "Pull request", meta: "#412", dotTone: "ochre" },
];

export const MOCK_DIFF_LINES = [
  { line: "18", text: "impl IndexWriter {", kind: "context" as const },
  { line: "−", text: "  pub fn flush(&mut self) -> Result<()> {", kind: "removed" as const },
  { line: "+", text: "  pub async fn flush(&mut self) -> Result<usize> {", kind: "added" as const },
  { line: "+", text: "    let batch = self.pending.drain(..);", kind: "added" as const },
  { line: "+", text: "    self.table.add(batch).await?;", kind: "added" as const },
  { line: "24", text: "  }", kind: "context" as const },
];

export const MOCK_SHIP_STEPS = ["Worktree", "Preview", "Review", "Commit", "PR", "Merge"];
