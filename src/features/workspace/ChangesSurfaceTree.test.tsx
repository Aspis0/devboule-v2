// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceGitFileDiff, WorkspaceGitRow, WorkspaceGitStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitStatus: vi.fn(),
  workspaceGitDiff: vi.fn(),
}));

import { workspaceGitDiff, workspaceGitStatus } from "../../lib/tauri";
import { ChangesSurface } from "./ChangesSurface";
import { assembleCssProof, removeCssProof } from "./cssProof";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const WORKSPACE = "workspace-changes-tree-subject";

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

describe("ChangesSurface R7b panel body", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply());
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply());
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    removeCssProof();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
  }

  function branchRow(): HTMLElement {
    const found = container.querySelector<HTMLElement>(".workspace-changes-branch");
    if (found === null) throw new Error("branch row did not render");
    return found;
  }

  it("shows the branch name with the exact total beside it", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        branch: "feature/redesign",
        totals: { additions: 96, deletions: 41 },
        rows: [row({ path: "src/a.ts", additions: 96, deletions: 41 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    expect(branchRow().textContent).toContain("feature/redesign");
    expect(branchRow().textContent).toContain("+96 −41");
    expect(branchRow().textContent).not.toContain("≈");
    // The chevron is display-only: branch switching is out of scope, so it
    // is a span, never a control that looks like it switches.
    expect(branchRow().querySelector(".workspace-changes-branch-chevron")?.tagName).toBe("SPAN");
  });

  it("prints a detached head and a missing branch honestly, never blank", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        branch: "(detached)",
        totals: { additions: 2, deletions: 0 },
        rows: [row({ path: "a.ts", additions: 2 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);
    expect(branchRow().textContent).toContain("(detached)");

    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        branch: null,
        totals: { additions: 2, deletions: 0 },
        rows: [row({ path: "a.ts", additions: 2 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);
    expect(branchRow().textContent).toContain("No branch");
  });

  it("marks the total estimated when a row is capped or the counts arrived with a caveat", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 96, deletions: 41 },
        rows: [row({ path: "src/a.ts", additions: 96, deletions: 41, capped: true })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);
    expect(branchRow().textContent).toContain("≈+96 −41");
  });

  it("renders nothing countable when the reply withholds its rows", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply({ dirty: true }));
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    expect(container.querySelector(".workspace-changes-tree")).toBeNull();
    expect(container.querySelector(".workspace-changes-branch-totals")).toBeNull();
    expect(container.textContent).not.toContain("+0 −0");
  });

  it("groups nested rows under folders with the subtree sums, collapsible by button and keys", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 11, deletions: 5 },
        rows: [
          row({ path: "src/a.ts", additions: 2, deletions: 1 }),
          row({ path: "src/sub/b.ts", additions: 9, deletions: 4 }),
        ],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const tree = container.querySelector('[role="tree"]');
    if (tree === null) throw new Error("tree did not render");
    expect(tree.textContent).toContain("src");
    expect(tree.textContent).toContain("+11 −5");
    expect(tree.textContent).toContain("b.ts");
    // The file's full path rides the row's title: the basename is display,
    // the path is what acts key on.
    const file = container.querySelector('.workspace-file-change[title="src/sub/b.ts"]');
    expect(file).not.toBeNull();

    const toggle = container.querySelector<HTMLButtonElement>('button[aria-expanded="true"]');
    if (toggle === null) throw new Error("folder toggle did not render");
    await act(async () => {
      toggle.click();
    });
    expect(container.querySelector('.workspace-file-change[title="src/sub/b.ts"]')).toBeNull();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    // The folder keeps its FULL subtree sums while collapsed.
    expect(toggle.parentElement?.textContent).toContain("+11 −5");

    await act(async () => {
      toggle.focus();
      toggle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    await act(async () => {
      toggle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector('.workspace-file-change[title="src/sub/b.ts"]')).not.toBeNull();
  });

  it("paints new files' stats in the add tone and keeps the select behaviour", async () => {
    vi.mocked(workspaceGitDiff).mockResolvedValue(
      diffReply({ path: "notes/todo.md", lines: [{ kind: "add", text: "new" }] }),
    );
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 7, deletions: 0 },
        rows: [row({ path: "notes/todo.md", additions: 7, status: "untracked" })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const stats = container.querySelector(".workspace-file-change-stats-is-add");
    if (stats === null) throw new Error("add-tone stats did not render");
    expect(stats.textContent).toBe("+7 −0");
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    expect(vi.mocked(workspaceGitDiff)).toHaveBeenCalledWith(WORKSPACE, "notes/todo.md");
    expect(container.querySelector(".workspace-diff-lines")).not.toBeNull();
  });

  it("keeps the Commits view an honest empty state with no controls", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [row({ path: "a.ts", additions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const commits = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Commits",
    );
    if (commits === undefined) throw new Error("Commits switch did not render");
    await act(async () => {
      commits.click();
    });
    expect(container.textContent).toContain("History of this branch will appear here");
    expect(container.querySelector(".workspace-file-change")).toBeNull();
    expect(container.querySelector(".workspace-commit-row")).toBeNull();

    const uncommitted = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Uncommitted",
    );
    if (uncommitted === undefined) throw new Error("Uncommitted switch did not render");
    await act(async () => {
      uncommitted.click();
    });
    expect(container.querySelector(".workspace-file-change")).not.toBeNull();
  });

  it("puts the commit row last with the message field, and refresh as a quiet icon button", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [row({ path: "a.ts", additions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const commitRow = container.querySelector(".workspace-commit-row");
    if (commitRow === null) throw new Error("commit row did not render");
    expect(commitRow.querySelector('[aria-label="Commit message"]')).not.toBeNull();
    expect(commitRow.textContent).toContain("Commit");
    // After the tree in DOM order: the commit row closes the panel.
    const tree = container.querySelector(".workspace-changes-tree");
    if (tree === null) throw new Error("tree did not render");
    expect(tree.compareDocumentPosition(commitRow)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    // No text Refresh control anywhere: one icon button with the label.
    expect(
      Array.from(container.querySelectorAll("button")).some(
        (candidate) => candidate.textContent === "Refresh",
      ),
    ).toBe(false);
    const refresh = container.querySelector('button[aria-label="Refresh"]');
    if (refresh === null) throw new Error("refresh icon button did not render");
    await act(async () => {
      refresh.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(2);
  });

  it("offers the slice-8 hand-off on the selected row only, never a dead pencil", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [row({ path: "a.ts", additions: 3 })],
      }),
    );
    const onOpenFile = vi.fn();
    await render(<ChangesSurface workspaceId={WORKSPACE} onOpenFile={onOpenFile} />);
    expect(container.querySelector('[aria-label="Open diff in a tab"]')).toBeNull();

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    const pencil = container.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]');
    if (pencil === null) throw new Error("pencil did not render on the selected row");
    await act(async () => {
      pencil.click();
    });
    expect(onOpenFile).toHaveBeenCalledWith(WORKSPACE, "a.ts");
  });

  it("paints rows, switch and totals from the real sheets in bundle order", async () => {
    const { inject, token } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/panel.css"),
      read("src/features/workspace/panel/changes.css"),
    ]);
    inject([
      ".workspace-changes-branch",
      ".workspace-changes-seg-button",
      ".workspace-changes-seg-button-is-on",
      ".workspace-changes-file",
      ".workspace-changes-folder",
      ".workspace-file-change-stats-is-add",
    ]);
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 9, deletions: 4 },
        rows: [
          row({ path: "src/a.ts", additions: 2, deletions: 4 }),
          row({ path: "notes/todo.md", additions: 7, status: "untracked" }),
        ],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const branch = branchRow();
    expect(getComputedStyle(branch).fontSize).toBe("12px");
    const active = Array.from(container.querySelectorAll(".workspace-changes-seg-button")).find(
      (candidate) => candidate.textContent === "Uncommitted",
    );
    if (active === undefined) throw new Error("Uncommitted switch did not render");
    expect(getComputedStyle(active as HTMLElement).height).toBe("24px");
    expect(getComputedStyle(active as HTMLElement).backgroundColor).toBe(token("--panel-card"));
    const fileRow = container.querySelector<HTMLElement>(".workspace-changes-file");
    if (fileRow === null) throw new Error("file row did not render");
    expect(getComputedStyle(fileRow).height).toBe("24px");
    const addStats = container.querySelector<HTMLElement>(".workspace-file-change-stats-is-add");
    if (addStats === null) throw new Error("add-tone stats did not render");
    expect(getComputedStyle(addStats).color).toBe(token("--tone-add"));
  });
});
