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
  let root: Root | undefined;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = undefined;
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply());
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply());
  });

  afterEach(async () => {
    const current = root;
    if (current !== undefined) {
      await act(async () => {
        current.unmount();
      });
    }
    container.remove();
    removeCssProof();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  async function render(ui: ReactNode) {
    // One root per container: a second createRoot orphans the first with a
    // live 5 s poll inside it, which lands in a later test on a slow day.
    const previous = root;
    if (previous !== undefined) {
      await act(async () => {
        previous.unmount();
      });
    }
    root = createRoot(container);
    const current = root;
    await act(async () => {
      current.render(ui);
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
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({ dirty: true, branch: "main", totals: { additions: 0, deletions: 0 } }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    // The header facts survive the cap (workspace_git_status.rs:60-63), so
    // the branch name stands — but no totals and no tree, never zeros.
    expect(branchRow().textContent).toContain("main");
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

    const tree = container.querySelector(".workspace-changes-tree");
    if (tree === null) throw new Error("tree did not render");
    // A disclosure list, not a tree: rows carry five tabbable controls
    // each, so the panel claims no single-tab-stop pattern it cannot keep.
    expect(container.querySelector('[role="tree"]')).toBeNull();
    expect(tree.textContent).toContain("src");
    expect(tree.textContent).toContain("+11 −5");
    expect(tree.textContent).toContain("b.ts");
    // The file's full path rides the row's title: the basename is display,
    // the path is what acts key on.
    const file = container.querySelector('.workspace-file-change[title="src/sub/b.ts"]');
    expect(file).not.toBeNull();

    const toggle = container.querySelector<HTMLButtonElement>('button[aria-expanded="true"]');
    if (toggle === null) throw new Error("folder toggle did not render");
    // The disclosure names the group it owns.
    const groupId = toggle.getAttribute("aria-controls");
    if (groupId === null) throw new Error("folder toggle names no group");
    expect(container.querySelector(`#${CSS.escape(groupId)}`)).not.toBeNull();
    // Collapse starts from OPEN: one click hides the children, the next
    // brings them back, and the folder keeps its FULL subtree sums shut.
    await act(async () => {
      toggle.click();
    });
    expect(container.querySelector('.workspace-file-change[title="src/sub/b.ts"]')).toBeNull();
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.parentElement?.textContent).toContain("+11 −5");
    await act(async () => {
      toggle.click();
    });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector('.workspace-file-change[title="src/sub/b.ts"]')).not.toBeNull();
  });

  it("leaves arrow keys to the row's own controls: Down on Stage stays on Stage", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [row({ path: "src/a.ts", additions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const stage = container.querySelector<HTMLButtonElement>('button[title="Stage src/a.ts"]');
    if (stage === null) throw new Error("Stage button did not render");
    await act(async () => {
      stage.focus();
      stage.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(stage);
  });

  it("indents file names 14px right of their folder, with the brief's file icon", async () => {
    // The mockup's row is [pad 6][indent 14/level][glyph 12][gap 6][name]:
    // a folder's name starts at 6+12+6 = 24px, its depth-1 file at
    // 6+14+12+6 = 38px. happy-dom does no layout, so this test pins every
    // number the x is made of: the depth pad (inline, like FilesSurface's
    // indent()), the 12px glyphs and the 6px gaps from the real sheet.
    const { inject } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/changes.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    inject([
      ".workspace-changes-folder",
      ".workspace-changes-file",
      ".workspace-changes-chevron",
      ".workspace-changes-file-icon",
    ]);
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [
          row({ path: "src/a.ts", additions: 1 }),
          row({ path: "src/sub/b.ts", additions: 2 }),
        ],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    const folder = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.getAttribute("aria-label") === "Collapse src",
    );
    if (folder === undefined) throw new Error("folder toggle did not render");
    expect(getComputedStyle(folder).paddingLeft).toBe("6px");
    expect(getComputedStyle(folder).gap).toBe("6px");
    const chevron = folder.querySelector<HTMLElement>(".workspace-changes-chevron");
    if (chevron === null) throw new Error("folder chevron did not render");
    expect(getComputedStyle(chevron).width).toBe("12px");

    const file = container.querySelector<HTMLButtonElement>(
      '.workspace-changes-file[title="src/sub/b.ts"]',
    );
    if (file === null) throw new Error("nested file row did not render");
    expect(getComputedStyle(file).paddingLeft).toBe("34px");
    expect(getComputedStyle(file).gap).toBe("6px");
    const sibling = container.querySelector<HTMLElement>(
      '.workspace-changes-file[title="src/a.ts"]',
    );
    if (sibling === null) throw new Error("depth-1 file row did not render");
    expect(getComputedStyle(sibling).paddingLeft).toBe("20px");
    const icon = file.querySelector<HTMLElement>(".workspace-changes-file-icon");
    if (icon === null) throw new Error("file icon did not render");
    expect(getComputedStyle(icon).width).toBe("12px");
    // Name x: folder 6+12+6 = 24px, file 34+12+6 = 52px at depth 2 —
    // one 14px step per level below its folder's 24px.
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

  it("marks the open file current, never a pressed toggle", async () => {
    // Selecting shows the diff below; pressing again changes nothing, so
    // the row must not announce a toggle contract (aria-pressed) it cannot
    // keep. aria-current names what the styling already says.
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [row({ path: "src/a.ts", additions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);
    expect(container.querySelector("[aria-current]")).toBeNull();

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    const selected = container.querySelector('.workspace-file-change[title="src/a.ts"]');
    expect(selected?.getAttribute("aria-current")).toBe("true");
    expect(container.querySelector(".workspace-changes-tree [aria-pressed]")).toBeNull();
  });

  it("marks folder sums estimated when the counts arrive with a caveat", async () => {
    // The branch total reads ≈ on a caveat (changesTotalsLabel); the tree
    // must agree — a degraded round floors every number it produced.
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        error: "git diff produced more than the reply cap; the line counts are a floor",
        totals: { additions: 9, deletions: 0 },
        rows: [row({ path: "src/a.ts", additions: 9 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    expect(branchRow().textContent).toContain("≈+9 −0");
    const folderStats = container.querySelector(".workspace-changes-folder-stats");
    if (folderStats === null) throw new Error("folder stats did not render");
    expect(folderStats.textContent).toBe("≈+9 −0");
  });

  it("keeps the tree standing while the commit field takes keystrokes", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [row({ path: "src/a.ts", additions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    // Collapse first: the state to preserve is one a re-render could lose.
    const toggle = container.querySelector<HTMLButtonElement>('button[aria-expanded="true"]');
    if (toggle === null) throw new Error("folder toggle did not render");
    await act(async () => {
      toggle.click();
    });
    const input = container.querySelector<HTMLInputElement>('[aria-label="Commit message"]');
    if (input === null) throw new Error("commit field did not render");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");
    await act(async () => {
      setter.call(input, "wip");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    // No re-read owed to typing, the collapse intact, the draft kept.
    expect(vi.mocked(workspaceGitStatus)).toHaveBeenCalledTimes(1);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(input.value).toBe("wip");
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
    // Last in the row, where the mockup puts it: after the stats the
    // totals pushed right and after the row's own acts.
    const rowEl = pencil.closest(".workspace-file-change-row");
    if (rowEl === null) throw new Error("pencil left its row");
    const controls = Array.from(rowEl.querySelectorAll("button"));
    expect(controls[controls.length - 1]).toBe(pencil);
    await act(async () => {
      pencil.click();
    });
    expect(onOpenFile).toHaveBeenCalledWith(WORKSPACE, "a.ts");
  });

  it("paints rows, switch and totals from the real sheets in bundle order", async () => {
    // Bundle order per the import graph: Workspace.css is pulled in by
    // NewProjectDialog (:15) before the registry chain (:16) pulls in
    // changes.css; strip and the panel chrome come later.
    const { inject, token } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/changes.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    inject([
      ".workspace-changes",
      ".workspace-changes-branch",
      ".workspace-changes-branch-totals",
      ".workspace-changes-refresh",
      ".workspace-changes-seg",
      ".workspace-changes-seg-button",
      ".workspace-changes-seg-button-is-on",
      ".workspace-changes-file",
      ".workspace-changes-folder",
      ".workspace-changes-folder-stats",
      ".workspace-file-change-stats-is-add",
      ".workspace-changes-pencil",
      ".workspace-commit-row",
      ".workspace-commit-button",
      // The bare header target pulls in BOTH the mono group rule and the
      // moved display rule; the scoped target pulls the Changes answer.
      // All three match one element, so this is the real cascade.
      ".workspace-diff-header",
      ".workspace-changes .workspace-diff-header",
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

    // The diff header sits under Workspace.css's mono group rule; the
    // scoped Changes rule must hold it at sans in this order.
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    const diffHeader = container.querySelector<HTMLElement>(".workspace-diff-header");
    if (diffHeader === null) throw new Error("diff header did not render");
    expect(getComputedStyle(diffHeader).fontFamily).not.toContain("JetBrains Mono");
  });

  it("pins the commit row to the panel bottom outside the scroll flow", async () => {
    const { inject } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/changes.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    inject([".workspace-changes", ".workspace-commit-row"]);
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({
        dirty: true,
        totals: { additions: 3, deletions: 0 },
        rows: [row({ path: "a.ts", additions: 3 })],
      }),
    );
    await render(<ChangesSurface workspaceId={WORKSPACE} />);

    // The root is a flex column carrying the row to the bottom; the row
    // itself sticks to the scrollport's bottom edge while the tree
    // scrolls under it. Whether it truly never scrolls away is live-only
    // (happy-dom does no layout), but the mechanism is pinned here.
    const root = container.querySelector<HTMLElement>(".workspace-changes");
    if (root === null) throw new Error("changes root did not render");
    expect(getComputedStyle(root).display).toBe("flex");
    const commitRow = container.querySelector<HTMLElement>(".workspace-commit-row");
    if (commitRow === null) throw new Error("commit row did not render");
    expect(getComputedStyle(commitRow).position).toBe("sticky");
    expect(getComputedStyle(commitRow).bottom).toBe("0px");
    const tree = container.querySelector(".workspace-changes-tree");
    if (tree === null) throw new Error("tree did not render");
    expect(tree.compareDocumentPosition(commitRow)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  });
});
