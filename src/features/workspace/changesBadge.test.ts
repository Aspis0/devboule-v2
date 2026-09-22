import { describe, expect, it, vi } from "vitest";
import type { WorkspaceGitStatus } from "../../types/ipc";
import { CHANGES_BADGE_UNREAD, changesBadge, changesBadgeLabel } from "./changesBadge";

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

describe("changesBadge store", () => {
  it("reports nothing for a workspace that was never read", () => {
    expect(changesBadge.snapshot("badge-never-read")).toBeNull();
    expect(changesBadge.snapshot(null)).toBeNull();
    expect(CHANGES_BADGE_UNREAD).toBe("—");
  });

  it("keeps the last label per workspace, so one checkout cannot show another's numbers", () => {
    changesBadge.report("badge-workspace-a", "+12 −3");
    changesBadge.report("badge-workspace-b", "clean");

    expect(changesBadge.snapshot("badge-workspace-a")).toBe("+12 −3");
    expect(changesBadge.snapshot("badge-workspace-b")).toBe("clean");
  });

  it("notifies on a new label and stays silent when the poll found nothing new", () => {
    const listener = vi.fn();
    const unsubscribe = changesBadge.subscribe(listener);
    try {
      changesBadge.report("badge-workspace-c", "+1 −1");
      expect(listener).toHaveBeenCalledTimes(1);
      // A 5 s poll that read the same label again must not re-render the
      // workspace for a value that did not change.
      changesBadge.report("badge-workspace-c", "+1 −1");
      expect(listener).toHaveBeenCalledTimes(1);
      changesBadge.report("badge-workspace-c", "clean");
      expect(listener).toHaveBeenCalledTimes(2);
    } finally {
      unsubscribe();
    }

    changesBadge.report("badge-workspace-c", "unavailable");
    expect(listener).toHaveBeenCalledTimes(2);
  });
});
