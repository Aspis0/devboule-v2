import { describe, expect, it } from "vitest";
import type { WorkspaceGitStatus } from "../../types/ipc";
import { changesBadgeLabel, changesBranchLabel, changesTotalsLabel } from "./changesBadge";

function status(overrides: Partial<WorkspaceGitStatus> = {}): WorkspaceGitStatus {
  return {
    isGit: true,
    dirty: false,
    branch: "main",
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
    ...overrides,
  };
}

const ROW = {
  path: "src/writer.ts",
  additions: 92,
  deletions: 41,
  status: "modified" as const,
  capped: false,
};

describe("changesBadgeLabel", () => {
  it("states the exact totals of an exact, healthy list", () => {
    // Kills the label for a plain dirty tree: the badge must be the reply's
    // own totals, not a placeholder and not an approximation.
    expect(
      changesBadgeLabel(
        status({ dirty: true, totals: { additions: 92, deletions: 41 }, rows: [ROW] }),
      ),
    ).toBe("+92 −41");
  });

  it("keeps the inexact mark when any count is a floor", () => {
    expect(
      changesBadgeLabel(
        status({
          dirty: true,
          totals: { additions: 92, deletions: 41 },
          rows: [ROW, { ...ROW, path: "notes/todo.md", capped: true }],
        }),
      ),
    ).toBe("≈+92 −41");
    expect(
      changesBadgeLabel(
        status({
          dirty: true,
          totals: { additions: 92, deletions: 41 },
          rows: [ROW],
          error: "git diff produced more than the reply cap; the line counts are a floor",
        }),
      ),
    ).toBe("≈+92 −41");
  });

  it("keeps 'not a repository' apart from an unavailable folder", () => {
    expect(changesBadgeLabel(status({ isGit: false }))).toBe("not a repo");
    expect(
      changesBadgeLabel(status({ isGit: false, error: "the workspace folder is not a directory" })),
    ).toBe("unavailable");
  });

  it("reports a dirty tree whose row list was withheld as changes, not as clean", () => {
    // The withheld list keeps `dirty` while dropping the rows: reading this as
    // "clean" would tell the user the opposite of what the reply said.
    expect(
      changesBadgeLabel(
        status({
          dirty: true,
          error: "git status produced more than the reply cap; the row list is withheld",
        }),
      ),
    ).toBe("changes");
  });

  it("says clean only for a reply that claims nothing is wrong", () => {
    expect(changesBadgeLabel(status())).toBe("clean");
    expect(
      changesBadgeLabel(status({ error: "this workspace folder is inside a git repository" })),
    ).toBe("unavailable");
  });
});

describe("changesTotalsLabel", () => {
  it("states the exact totals of an exact list", () => {
    expect(
      changesTotalsLabel(
        status({ dirty: true, totals: { additions: 96, deletions: 41 }, rows: [ROW] }),
      ),
    ).toBe("+96 −41");
  });

  it("keeps the inexact mark when any count is a floor", () => {
    expect(
      changesTotalsLabel(
        status({
          dirty: true,
          totals: { additions: 96, deletions: 41 },
          rows: [{ ...ROW, capped: true }],
        }),
      ),
    ).toBe("≈+96 −41");
  });

  it("reads nothing without rows: clean, withheld or rowless caveat alike", () => {
    expect(changesTotalsLabel(status())).toBeNull();
    expect(changesTotalsLabel(status({ dirty: true }))).toBeNull();
    expect(changesTotalsLabel(status({ error: "git status exited with code 128" }))).toBeNull();
  });
});

describe("changesBranchLabel", () => {
  it("prints the wire's name verbatim and a missing one honestly", () => {
    expect(changesBranchLabel("feature/redesign")).toBe("feature/redesign");
    expect(changesBranchLabel("(detached)")).toBe("(detached)");
    expect(changesBranchLabel(null)).toBe("No branch");
  });
});
