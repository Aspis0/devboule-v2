// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkspaceGitCommitEntry,
  WorkspaceGitFileDiff,
  WorkspaceGitLog,
  WorkspaceGitRow,
  WorkspaceGitStatus,
} from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitStatus: vi.fn(),
  workspaceGitDiff: vi.fn(),
  workspaceGitLog: vi.fn(),
}));

import { workspaceGitDiff, workspaceGitLog, workspaceGitStatus } from "../../lib/tauri";
import { ChangesSurface } from "./ChangesSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-changes-subject";

function commitEntry(overrides: Partial<WorkspaceGitCommitEntry> = {}): WorkspaceGitCommitEntry {
  return {
    sha: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
    shortSha: "a1b2c3d",
    subject: "Add the thing",
    authorName: "gualt",
    authorDate: "2026-09-01T10:00:00+00:00",
    isOnRemote: true,
    isOnBase: false,
    ...overrides,
  };
}

function logReply(overrides: Partial<WorkspaceGitLog> = {}): WorkspaceGitLog {
  return {
    baseRef: "origin/main",
    commits: [commitEntry()],
    error: null,
    ...overrides,
  };
}

function statusReply(overrides: Partial<WorkspaceGitStatus> = {}): WorkspaceGitStatus {
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

function row(overrides: Partial<WorkspaceGitRow> & { path: string }): WorkspaceGitRow {
  return { additions: 0, deletions: 0, status: "modified", capped: false, ...overrides };
}

function diffReply(overrides: Partial<WorkspaceGitFileDiff> = {}): WorkspaceGitFileDiff {
  return {
    path: "src/writer.ts",
    isNew: false,
    isDeleted: false,
    additions: 0,
    deletions: 0,
    lines: [],
    status: "ok",
    error: null,
    ...overrides,
  };
}

/** A promise the test settles itself, so a pending state is asserted, not raced. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("ChangesSurface", () => {
  let container: HTMLDivElement;
  let root: Root;

  /** The Commits segment — the switch's second button, by its label. */
  function commitsSegment(): HTMLButtonElement {
    const found = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Commits",
    );
    if (found === undefined) throw new Error("Commits switch did not render");
    return found;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply());
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply());
    vi.mocked(workspaceGitLog).mockResolvedValue(logReply());
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
  }

  function controls(): (string | null)[] {
    return Array.from(container.querySelectorAll("button, [role='button']")).map(
      (control) => control.textContent,
    );
  }

  it("holds a loading state until the first status answer arrives", async () => {
    const pending = deferred<WorkspaceGitStatus>();
    vi.mocked(workspaceGitStatus).mockReturnValue(pending.promise);
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Loading changes…");
    expect(container.querySelector(".workspace-file-change")).toBeNull();

    await act(async () => {
      pending.resolve(statusReply());
    });
    expect(container.textContent).toContain("No uncommitted changes in this workspace.");
  });

  it("labels a folder that is not a repository as its own state, not an error", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply({ isGit: false }));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(container.textContent).toContain("This workspace folder is not a git repository.");
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.textContent).not.toContain("No uncommitted changes");
  });

  it("says the tree is clean and keeps the refresh action there", async () => {
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    // Anchored on the panel's own words first: the absence assertions below
    // must not be satisfiable by a panel that renders nothing.
    expect(container.textContent).toContain("No uncommitted changes in this workspace.");
    expect(container.querySelector('button[aria-label="Refresh"]')).not.toBeNull();
    expect(container.textContent).not.toContain("Mockup");
    expect(container.querySelector('[role="note"]')).toBeNull();
  });

  // Kills mutation (c) — `caveatOf` returning null: the wire's sentence must
  // be shown as it arrived, and the clean-tree claim must not replace it.
  it("shows the wire's caveat sentence instead of claiming anything about the tree", async () => {
    const caveat = "git status exited with code 128";
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply({ error: caveat }));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(caveat);
    expect(container.textContent).not.toContain("No uncommitted changes");
    expect(container.textContent).not.toContain("not a git repository");
    expect(container.querySelector(".workspace-file-change")).toBeNull();
    // The caveat reply names the checkout (isGit, branch kept) but withholds
    // every count: the branch name and a retry stand beside the sentence.
    expect(container.querySelector(".workspace-changes-branch")?.textContent).toContain("main");
    expect(container.querySelector(".workspace-changes-branch-totals")).toBeNull();
    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render on caveat");
    await act(async () => {
      refresh.click();
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
  });

  it("names a missing branch honestly on a caveat reply, with a retry", async () => {
    // The daemon's caveat() carries branch: None — no name invented.
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({ branch: null, error: "git status exited with code 128" }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(container.querySelector(".workspace-changes-branch")?.textContent).toContain(
      "No branch",
    );
    expect(container.querySelector('button[aria-label="Refresh"]')).not.toBeNull();
  });

  // Fix round R1: a degraded count round brings rows AND the sentence
  // (`workspace_git_status.rs:74-83`). Kills the mutation that drops the list
  // from under the alert — one or the other alone is not the behaviour.
  it("keeps the list on screen under the sentence that distrusts its counts", async () => {
    const caveat =
      "git diff produced more than the reply cap; the line counts are a floor, not a count";
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        error: caveat,
        totals: { additions: 92, deletions: 41 },
        rows: [
          row({
            path: "crates/devboule-daemon/src/workspace_git_status.rs",
            additions: 92,
            deletions: 41,
            capped: true,
          }),
        ],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(caveat);
    expect(container.querySelectorAll(".workspace-file-change")).toHaveLength(1);
    expect(
      container.querySelector(
        '.workspace-file-change[title="crates/devboule-daemon/src/workspace_git_status.rs"]',
      ),
    ).not.toBeNull();
    expect(container.textContent).toContain("workspace_git_status.rs");
    expect(container.textContent).toContain("≈+92 −41");
    expect(container.textContent).not.toContain("No uncommitted changes");
  });

  // Kills mutation (a) — hardcoded rows in place of the reply's — and
  // mutation (e) — the `≈` mark dropped from a `capped` row's counts.
  it("renders the reply's rows with counts, status words and the capped mark", async () => {
    const rows = [
      row({
        path: "crates/devboule-daemon/src/workspace_git_status.rs",
        additions: 92,
        deletions: 41,
      }),
      row({ path: "notes/todo.md", additions: 7, status: "untracked", capped: true }),
      row({ path: "src/writer.ts", deletions: 12, status: "deleted" }),
    ];
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 99, deletions: 53 },
        rows,
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(container.querySelectorAll(".workspace-file-change")).toHaveLength(3);
    expect(
      container.querySelector(
        '.workspace-file-change[title="crates/devboule-daemon/src/workspace_git_status.rs"]',
      ),
    ).not.toBeNull();
    expect(container.querySelector('.workspace-file-change[title="notes/todo.md"]')).not.toBeNull();
    expect(container.querySelector('.workspace-file-change[title="src/writer.ts"]')).not.toBeNull();
    expect(container.textContent).toContain("workspace_git_status.rs");
    expect(container.textContent).toContain("todo.md");
    expect(container.textContent).toContain("+92 −41");
    expect(container.textContent).toContain("untracked");
    expect(container.textContent).toContain("deleted");
    // `capped` says the counts are not exact: a mark, never an invented number.
    // The whole row's text, marks included — dropping `≈` changes this string.
    // The row shows the basename; the full path rides its `title`.
    const cappedRow = Array.from(container.querySelectorAll(".workspace-file-change")).find(
      (element) => element.getAttribute("title") === "notes/todo.md",
    );
    if (cappedRow === undefined) throw new Error("capped row did not render");
    expect(cappedRow.textContent).toBe("todo.mduntracked≈+7 −0");
    expect(
      container.querySelector('.workspace-file-change[title="src/writer.ts"]')?.className,
    ).toContain("workspace-file-change-muted");
    expect(container.textContent).not.toContain("Mockup");
  });

  it("shows a failed read's own sentence, distinct from 'not a repository'", async () => {
    const failure = "the app did not answer";
    vi.mocked(workspaceGitStatus).mockRejectedValue(new Error(failure));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(failure);
    expect(container.textContent).not.toContain("not a git repository");
    expect(container.textContent).not.toContain("No uncommitted changes");
  });

  // Fix round R2: mounted with A, answered with A, then the id changes to B —
  // nothing of A may be on screen under B. Kills the two guards' mutations:
  // dropping the workspace from the status cell or from the selection.
  it("shows nothing of the previous workspace once the id changes", async () => {
    const pendingB = deferred<WorkspaceGitStatus>();
    vi.mocked(workspaceGitStatus).mockImplementation((workspaceId: string) =>
      workspaceId === "workspace-changes-a"
        ? Promise.resolve(
            statusReply({
              dirty: true,
              totals: { additions: 5, deletions: 1 },
              rows: [row({ path: "a.ts", additions: 5, deletions: 1 })],
            }),
          )
        : pendingB.promise,
    );
    vi.mocked(workspaceGitDiff).mockResolvedValue(
      diffReply({
        path: "a.ts",
        additions: 5,
        deletions: 1,
        lines: [{ kind: "add", text: "export const a = 1;" }],
      }),
    );
    await render(<ChangesSurface workspaceId="workspace-changes-a" canListCommits={true} />);

    // Anchor: A's list, A's selected row and A's diff are on screen.
    expect(container.textContent).toContain("a.ts");
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    expect(container.querySelector('[aria-current="true"]')).not.toBeNull();
    expect(container.querySelector(".workspace-diff-lines")).not.toBeNull();

    // Switch to B, whose answer has not arrived yet.
    await act(async () => {
      root.render(<ChangesSurface workspaceId="workspace-changes-b" canListCommits={true} />);
    });
    expect(container.textContent).toContain("Loading changes…");
    expect(container.textContent).not.toContain("a.ts");
    expect(container.querySelector('[aria-current="true"]')).toBeNull();
    expect(container.querySelector(".workspace-diff-card")).toBeNull();

    await act(async () => {
      pendingB.resolve(statusReply());
    });
    expect(container.textContent).toContain("No uncommitted changes in this workspace.");
    expect(container.textContent).not.toContain("a.ts");
  });

  // Rewritten with the doctrine it asserts (§2.5.7): the owner reopened
  // DECISIONS §4 on 2026-09-22, so Stage/Unstage/Discard/Commit have the
  // right to exist here — the absences that remain mandatory are the
  // invented test result (kills mutation (d), the `cargo test · 142 passed`
  // card) and Stash, which was never implemented. Anchored on the real
  // rows first, so none of it can pass on an empty panel.
  it("offers the write actions the owner approved and no invented test result", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 14, deletions: 3 },
        rows: [row({ path: "src/real-change.ts", additions: 14, deletions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    // Anchor: the real reply's file must be on screen first, by its title —
    // the tree shows basenames — so the absence assertions cannot pass on
    // an empty panel.
    expect(
      container.querySelector('.workspace-file-change[title="src/real-change.ts"]'),
    ).not.toBeNull();
    expect(container.textContent).toContain("real-change.ts");
    expect(container.textContent).toContain("+14 −3");
    const labels = controls();
    expect(container.querySelector('button[aria-label="Refresh"]')).not.toBeNull();
    expect(labels).toContain("Stage");
    expect(labels).toContain("Unstage");
    expect(labels).toContain("Commit");
    expect(labels).not.toContain("Stash");
    expect(container.textContent).not.toContain("142 passed");
    expect(container.textContent).not.toContain("cargo test");
    expect(container.querySelector(".workspace-test-card")).toBeNull();
  });

  it("asks for the diff of the row that is selected and renders its four line kinds", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 1 },
        rows: [row({ path: "src/writer.ts", additions: 3, deletions: 1 })],
      }),
    );
    vi.mocked(workspaceGitDiff).mockResolvedValue(
      diffReply({
        additions: 3,
        deletions: 1,
        lines: [
          { kind: "header", text: "@@ -1,4 +1,6 @@" },
          { kind: "context", text: "export function run() {" },
          { kind: "remove", text: "  return 1;" },
          { kind: "add", text: "  return 2;" },
        ],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    const first = container.querySelector<HTMLButtonElement>(".workspace-file-change");
    if (first === null) throw new Error("row did not render");
    await act(async () => {
      first.click();
    });

    expect(vi.mocked(workspaceGitDiff).mock.calls[0]).toEqual([WORKSPACE, "src/writer.ts"]);
    expect(first.getAttribute("aria-current")).toBe("true");
    expect(container.querySelector(".workspace-diff-header span:first-child")?.textContent).toBe(
      "src/writer.ts",
    );
    expect(container.querySelector(".workspace-diff-header span:last-child")?.textContent).toBe(
      "+3 −1",
    );
    expect(container.querySelector(".workspace-diff-hunk")?.textContent).toContain(
      "@@ -1,4 +1,6 @@",
    );
    expect(container.querySelector(".workspace-diff-context")?.textContent).toContain(
      "export function run() {",
    );
    expect(container.querySelector(".workspace-diff-removed")?.textContent).toBe(
      "−removed  return 1;",
    );
    expect(container.querySelector(".workspace-diff-added")?.textContent).toBe("+added  return 2;");
  });

  // Fix round R3: the diff effect owns the FIRST read of a selection — at
  // activation, not at the first interval tick — so a remembered selection is
  // re-read the moment its workspace is current again. Kills the mutation that
  // drops the immediate `tick()` from that effect.
  it("puts a diff request in flight the moment a remembered selection is current again", async () => {
    vi.mocked(workspaceGitStatus).mockImplementation(async (workspaceId: string) =>
      workspaceId === "workspace-changes-a"
        ? statusReply({ dirty: true, rows: [row({ path: "a.ts" })] })
        : statusReply(),
    );
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply({ path: "a.ts" }));
    await render(<ChangesSurface workspaceId="workspace-changes-a" canListCommits={true} />);
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    const readsOfA = () =>
      vi.mocked(workspaceGitDiff).mock.calls.filter((call) => call[0] === "workspace-changes-a")
        .length;
    expect(readsOfA()).toBe(1);

    // Away and back, with the selection remembered: no timer is advanced, so
    // the only way a second request exists is the effect starting it now.
    const pending = deferred<WorkspaceGitFileDiff>();
    vi.mocked(workspaceGitDiff).mockReturnValue(pending.promise);
    await act(async () => {
      root.render(<ChangesSurface workspaceId="workspace-changes-b" canListCommits={true} />);
    });
    await act(async () => {
      root.render(<ChangesSurface workspaceId="workspace-changes-a" canListCommits={true} />);
    });

    expect(readsOfA()).toBe(2);
    await act(async () => {
      pending.resolve(
        diffReply({
          path: "a.ts",
          additions: 1,
          lines: [{ kind: "add", text: "fresh();" }],
        }),
      );
    });
    expect(container.textContent).toContain("fresh();");
    expect(container.textContent).not.toContain("Loading diff…");
  });

  it("holds a loading state for a selected file until its diff arrives", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({ dirty: true, rows: [row({ path: "src/writer.ts" })] }),
    );
    const pending = deferred<WorkspaceGitFileDiff>();
    vi.mocked(workspaceGitDiff).mockReturnValue(pending.promise);
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    const first = container.querySelector<HTMLButtonElement>(".workspace-file-change");
    if (first === null) throw new Error("row did not render");
    await act(async () => {
      first.click();
    });

    expect(container.querySelector(".workspace-diff-note")?.textContent).toBe("Loading diff…");
    expect(container.querySelector(".workspace-diff-lines")).toBeNull();

    await act(async () => {
      pending.resolve(diffReply());
    });
    expect(container.textContent).toContain("This file has no uncommitted line changes.");
  });

  it("reports a binary file as its own answer, with no lines and no flag claims", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({ dirty: true, rows: [row({ path: "assets/logo.png" })] }),
    );
    vi.mocked(workspaceGitDiff).mockResolvedValue(
      diffReply({ path: "assets/logo.png", status: "binary" }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });

    expect(container.textContent).toContain("This file is binary; there are no lines to show.");
    expect(container.querySelector(".workspace-diff-header span:last-child")?.textContent).toBe(
      "binary",
    );
    expect(container.querySelector(".workspace-diff-lines")).toBeNull();
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("shows the sentence that names the cap that withheld a too-large diff", async () => {
    const cap = "the diff is over the 1 MiB per-file cap, so the lines are withheld";
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({ dirty: true, rows: [row({ path: "vendor/big.ts" })] }),
    );
    vi.mocked(workspaceGitDiff).mockResolvedValue(
      diffReply({ path: "vendor/big.ts", status: "too_large", error: cap }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(cap);
    expect(container.querySelector(".workspace-diff-header span:last-child")?.textContent).toBe(
      "too large",
    );
    expect(container.querySelector(".workspace-diff-lines")).toBeNull();
  });

  it("shows a refused diff as the wire's sentence, not as an empty diff", async () => {
    const refusal = "this path is not tracked by git and has no uncommitted content";
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({ dirty: true, rows: [row({ path: ".gitignore" })] }),
    );
    vi.mocked(workspaceGitDiff).mockResolvedValue(
      diffReply({ path: ".gitignore", status: "error", error: refusal }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(refusal);
    expect(container.querySelector(".workspace-diff-header span:last-child")?.textContent).toBe(
      "error",
    );
    expect(container.querySelector(".workspace-diff-lines")).toBeNull();
  });

  it("re-reads every 5 seconds while the panel is open", async () => {
    vi.useFakeTimers();
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(3);
  });

  it("stops reading once the panel is unmounted", async () => {
    vi.useFakeTimers();
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    // Dropping the panel from the tree runs the same cleanup as closing it.
    await act(async () => {
      root.render(null);
    });

    await act(async () => {
      vi.advanceTimersByTime(20_000);
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
  });

  it("reads again on the manual refresh button", async () => {
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);

    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render");
    await act(async () => {
      refresh.click();
    });

    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
  });

  // Fix round R6 (DECISIONS §10): a refused read is not a reading — the panel
  // keeps showing the last reply actually read, beside the refusal.
  it("keeps the last reply on screen when a later read is refused", async () => {
    const id = "workspace-badge-refused";
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 14, deletions: 3 },
        rows: [row({ path: "src/real.ts", additions: 14, deletions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={id} canListCommits={true} />);
    expect(container.querySelector('.workspace-file-change[title="src/real.ts"]')).not.toBeNull();

    vi.mocked(workspaceGitStatus).mockRejectedValue(new Error("the daemon refused this read"));
    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render");
    await act(async () => {
      refresh.click();
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "the daemon refused this read",
    );
    expect(container.querySelector('.workspace-file-change[title="src/real.ts"]')).not.toBeNull();
  });

  it("asks nothing while no workspace is selected and says so", async () => {
    await render(<ChangesSurface workspaceId={null} canListCommits={true} />);

    expect(vi.mocked(workspaceGitStatus)).not.toHaveBeenCalled();
    expect(container.textContent).toContain("No workspace is selected.");
  });

  // Fix round R5: with no workspace there is nothing to refresh, so the
  // control is not drawn at all. Kills the mutation that renders it anyway.
  it("offers no refresh control while no workspace is selected", async () => {
    await render(<ChangesSurface workspaceId={null} canListCommits={true} />);

    // Anchored on the panel's own words, so the absence cannot pass on an
    // empty panel.
    expect(container.textContent).toContain("No workspace is selected.");
    expect(controls()).not.toContain("Refresh");
    expect(container.querySelector('button[aria-label="Refresh"]')).toBeNull();
  });

  // R7b fix pass 1: Refresh exists on every screen with a workspace — the
  // refused first read is exactly when a manual retry matters, and the 5 s
  // poll is no substitute when the reads themselves are what fails.
  it("keeps a refresh control on a refused first read", async () => {
    vi.mocked(workspaceGitStatus).mockRejectedValue(new Error("the daemon is restarting"));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);

    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render on refusal");
    await act(async () => {
      refresh.click();
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
  });

  it("keeps a refresh control while the first read is still in flight", async () => {
    const pending = deferred<WorkspaceGitStatus>();
    vi.mocked(workspaceGitStatus).mockReturnValue(pending.promise);
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    expect(container.querySelector('[role="status"]')?.textContent).toBe("Loading changes…");

    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render while loading");
    await act(async () => {
      refresh.click();
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);

    await act(async () => {
      pending.resolve(statusReply());
    });
    expect(container.textContent).toContain("No uncommitted changes in this workspace.");
  });

  it("keeps a refresh control when the folder is not a repository", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply({ isGit: false }));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    expect(container.textContent).toContain("This workspace folder is not a git repository.");

    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render off-repo");
    await act(async () => {
      refresh.click();
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
  });

  // R7d: the Commits segment that used to be an honest empty state is
  // wired to the history read. The list renders the branch's own
  // commits — Paseo's filter — and the base half stays out of it.
  it("renders the Commits view from the bridge answer", async () => {
    vi.mocked(workspaceGitLog).mockResolvedValue(
      logReply({
        commits: [
          commitEntry({ shortSha: "a1b2c3d", subject: "Add the thing", authorName: "gualt" }),
          commitEntry({
            sha: "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3",
            shortSha: "b2c3d4e",
            subject: "Base history",
            isOnBase: true,
          }),
        ],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      commitsSegment().click();
    });

    expect(container.querySelector(".workspace-commits-row")).not.toBeNull();
    expect(container.textContent).toContain("a1b2c3d");
    expect(container.textContent).toContain("Add the thing");
    expect(container.textContent).toContain("gualt");
    // The base branch's history is filtered out, not dimmed.
    expect(container.textContent).not.toContain("Base history");
    // The tree and the commit row stay on the Uncommitted side.
    expect(container.querySelector(".workspace-file-change")).toBeNull();
    expect(container.querySelector(".workspace-commit-row")).toBeNull();
  });

  // The wire's refusal arm: an empty list WITH a sentence. The daemon's
  // own words, alone — never the empty-tree claim.
  it("shows a wire refusal as the daemon's sentence, alone", async () => {
    const refusal = "the base branch main is not local and not on origin";
    vi.mocked(workspaceGitLog).mockResolvedValue(logReply({ commits: [], error: refusal }));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      commitsSegment().click();
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(refusal);
    expect(container.querySelector(".workspace-commits-row")).toBeNull();
    expect(container.querySelector(".workspace-commits-state")).toBeNull();
    expect(container.textContent).not.toContain("No commits ahead");
  });

  it("says the branch sits at its base when the answer carries no work", async () => {
    vi.mocked(workspaceGitLog).mockResolvedValue(
      logReply({ commits: [commitEntry({ shortSha: "b2c3d4e", isOnBase: true })] }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      commitsSegment().click();
    });

    expect(container.querySelector(".workspace-commits-state")?.textContent).toBe(
      "No commits ahead of origin/main",
    );
    expect(container.querySelector(".workspace-commits-row")).toBeNull();
  });

  it("shows a cut-short list beside the sentence that cut it", async () => {
    const cut = "git log produced more than the reply cap; the oldest commits are missing";
    vi.mocked(workspaceGitLog).mockResolvedValue(logReply({ error: cut }));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      commitsSegment().click();
    });

    expect(container.querySelector(".workspace-commits-row")).not.toBeNull();
    expect(container.querySelector(".workspace-commits-note")?.textContent).toBe(cut);
  });

  it("shows a refused history read as the daemon's sentence", async () => {
    vi.mocked(workspaceGitLog).mockRejectedValue(new Error("the folder is not a repository"));
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);

    await act(async () => {
      commitsSegment().click();
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "the folder is not a repository",
    );
    expect(container.querySelector(".workspace-commits-row")).toBeNull();
  });

  // Paseo's gate: the section is hidden unless the capability is
  // present. A daemon that cannot list history leaves the switch with
  // one segment — and a hidden segment reads nothing.
  it("hides the Commits segment when the daemon cannot list history", async () => {
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={false} />);

    expect(() => commitsSegment()).toThrow();
    expect(container.textContent).not.toContain("Commits");
    expect(container.textContent).toContain("No uncommitted changes in this workspace.");
    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();
  });

  // The poll is armed only while the list itself is mounted: a folder
  // that ceases to be a repository unmounts the switch, and the 30 s
  // read stops with it.
  it("stops the history poll when the folder ceases to be a repository", async () => {
    vi.useFakeTimers();
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    await act(async () => {
      commitsSegment().click();
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);

    // The folder stops being a repository; Refresh re-reads the status
    // (and the history, once) before the switch unmounts.
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply({ isGit: false }));
    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render");
    await act(async () => {
      refresh.click();
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(2);
    expect(container.querySelector(".workspace-changes-seg")).toBeNull();

    await act(async () => {
      vi.advanceTimersByTime(90_000);
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(2);
  });

  it("refreshes the history beside the status on the panel's refresh button", async () => {
    await render(<ChangesSurface workspaceId={WORKSPACE} canListCommits={true} />);
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);

    const refresh = container.querySelector<HTMLButtonElement>('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh button did not render");
    await act(async () => {
      refresh.click();
    });

    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);
  });
});
