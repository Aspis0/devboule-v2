// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceGitFileDiff, WorkspaceGitRow, WorkspaceGitStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitStatus: vi.fn(),
  workspaceGitDiff: vi.fn(),
  workspaceGitStage: vi.fn(),
  workspaceGitUnstage: vi.fn(),
  workspaceGitDiscard: vi.fn(),
  workspaceGitCommit: vi.fn(),
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
}));

// The confirmation belongs to the one act that loses data: the discard's
// gate lives inside `discard`, and this mock answers `false` on purpose —
// were the gate ever dropped from that road, the No-answers-nothing case
// below would die first. Stage, unstage and commit must never reach it at
// all, which their own assertions here prove.
vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(async () => false),
}));

import { confirm } from "@tauri-apps/plugin-dialog";
import {
  workspaceGitCommit,
  workspaceGitDiff,
  workspaceGitDiscard,
  workspaceGitStage,
  workspaceGitStatus,
  workspaceGitUnstage,
} from "../../lib/tauri";
import { ChangesSurface } from "./ChangesSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-git-actions-subject";
const ROW_PATH = "notes/todo.md";

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
  return { additions: 0, deletions: 0, status: "untracked", capped: false, ...overrides };
}

function diffReply(overrides: Partial<WorkspaceGitFileDiff> = {}): WorkspaceGitFileDiff {
  return {
    path: ROW_PATH,
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

/** One dirty row on screen — the state every act below starts from. */
function dirtyReply(): WorkspaceGitStatus {
  return statusReply({
    dirty: true,
    totals: { additions: 3, deletions: 0 },
    rows: [row({ path: ROW_PATH, additions: 3 })],
  });
}

describe("ChangesSurface actions", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceGitStatus).mockResolvedValue(dirtyReply());
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply());
    vi.mocked(workspaceGitStage).mockResolvedValue(null);
    vi.mocked(workspaceGitUnstage).mockResolvedValue(null);
    vi.mocked(workspaceGitDiscard).mockResolvedValue(null);
    vi.mocked(workspaceGitCommit).mockResolvedValue(null);
    vi.mocked(confirm).mockResolvedValue(false);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
  }

  function button(selector: string): HTMLButtonElement {
    const found = container.querySelector<HTMLButtonElement>(selector);
    if (found === null) throw new Error(`no button: ${selector}`);
    return found;
  }

  /** The toolbar's Commit button, found by its own label (the only one). */
  function commitButton(): HTMLButtonElement {
    const found = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Commit",
    );
    if (found === undefined) throw new Error("no commit button");
    return found;
  }

  /** Open a row's menu — the only way to Discard exists behind it. */
  async function openMenu(path: string = ROW_PATH) {
    await act(async () => {
      button(`button[aria-label="${path} actions"]`).click();
    });
    expect(container.querySelector('[role="menu"]')).not.toBeNull();
  }

  // The brief's §2.4 form, on the discard: mock `confirm` → false → wire
  // spy not called, the row intact, and NOTHING refreshed (a No means
  // nothing happened, so even the status re-read would be a lie). Then
  // true → called exactly once, with this row's path, and the immediate
  // refresh the brief demands. Mutation that kills the first half: drop
  // the gate from `discard` (call the command directly) — the false case
  // fails on the very first `not.toHaveBeenCalled`.
  it("a declined confirmation reaches no wire and changes nothing, an accepted one acts once", async () => {
    await render(<ChangesSurface workspaceId={WORKSPACE} />);
    expect(container.textContent).toContain(ROW_PATH);

    await openMenu();
    await act(async () => {
      container.querySelector<HTMLElement>('[role="menuitem"]')?.click();
    });

    expect(vi.mocked(confirm)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitDiscard)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain(ROW_PATH);

    vi.mocked(confirm).mockResolvedValue(true);
    await openMenu();
    await act(async () => {
      container.querySelector<HTMLElement>('[role="menuitem"]')?.click();
    });

    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledWith(WORKSPACE, [ROW_PATH]);
    // The immediate refresh after the act — the second status read exists
    // without any timer being advanced (the poll is 5 s; this test runs
    // in milliseconds).
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
  });

  // The brief's second UI case: after Stage the panel re-reads at once and
  // the row's own answer changes with it — first reply has the row, second
  // (the refresh's) does not. Mutation that kills it: drop the `refresh()`
  // from the writer hook — the second call never happens and the row never
  // leaves. Stage itself asks no confirmation: only discard does.
  it("stages without asking and re-reads the status immediately, not on the poll", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(dirtyReply());
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(statusReply());
    await render(<ChangesSurface workspaceId={WORKSPACE} />);
    expect(container.textContent).toContain(ROW_PATH);

    await act(async () => {
      button(`button[title="Stage ${ROW_PATH}"]`).click();
    });

    expect(vi.mocked(confirm)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceGitStage)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitStage)).toHaveBeenCalledWith(WORKSPACE, [ROW_PATH]);
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("No uncommitted changes in this workspace.");
    expect(container.textContent).not.toContain(ROW_PATH);
  });

  // The review's §4.1 defect, fixed end to end from the row down to the
  // wire: a renamed row acts on **both** of its paths, because the new one
  // alone leaves the old side's deletion staged and the only copy on disk
  // deleted — a half operation that answered success (measured on git
  // 2.54.0). The old path is the status reply's own `renamedFrom` (the
  // bare `-z` token the parse used to drop), and the confirmation names
  // both files it will touch. Mutant `e:1` — drop the partner from
  // `pathsOf`: the wire spy below receives a one-element array and the
  // first equality fails.
  it("sends both paths of a renamed row to the wire, stage and discard alike", async () => {
    const renamed: WorkspaceGitRow = {
      path: "notes/todo-v2.md",
      renamedFrom: "notes/todo-v1.md",
      additions: 3,
      deletions: 0,
      status: "renamed",
      capped: false,
    };
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [renamed],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);
    expect(container.textContent).toContain("notes/todo-v2.md");

    await act(async () => {
      button('button[title="Stage notes/todo-v2.md"]').click();
    });
    expect(vi.mocked(workspaceGitStage)).toHaveBeenCalledWith(WORKSPACE, [
      "notes/todo-v2.md",
      "notes/todo-v1.md",
    ]);

    vi.mocked(confirm).mockResolvedValue(true);
    await openMenu("notes/todo-v2.md");
    await act(async () => {
      container.querySelector<HTMLElement>('[role="menuitem"]')?.click();
    });

    expect(vi.mocked(workspaceGitDiscard)).toHaveBeenCalledWith(WORKSPACE, [
      "notes/todo-v2.md",
      "notes/todo-v1.md",
    ]);
    // The question itself names the file the user never clicked: both
    // sides are about to disappear.
    expect(vi.mocked(confirm)).toHaveBeenCalledWith(
      expect.stringContaining("notes/todo-v1.md"),
      expect.anything(),
    );
  });

  // Commit's own discipline at the keyboard: an empty message never
  // reaches the wire (the button is disabled for it — the daemon refuses
  // it again, but the panel should not offer the trip), a written one is
  // sent verbatim, and a landed commit clears the field so the next one
  // starts honest.
  it("refuses an empty commit message at the toolbar and sends a written one verbatim", async () => {
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const commit = commitButton();
    expect(commit.disabled).toBe(true);
    await act(async () => {
      commit.click();
    });
    expect(vi.mocked(workspaceGitCommit)).not.toHaveBeenCalled();

    const input = container.querySelector<HTMLInputElement>('[aria-label="Commit message"]');
    if (input === null) throw new Error("no message field");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");
    await act(async () => {
      setter.call(input, "say what changed");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    expect(commitButton().disabled).toBe(false);
    await act(async () => {
      commitButton().click();
    });

    expect(vi.mocked(workspaceGitCommit)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitCommit)).toHaveBeenCalledWith(WORKSPACE, "say what changed");
    expect(vi.mocked(confirm)).not.toHaveBeenCalled();
    expect(container.querySelector<HTMLInputElement>('[aria-label="Commit message"]')?.value).toBe(
      "",
    );
  });
});
