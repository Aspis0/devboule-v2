// @vitest-environment happy-dom

// The rows a keystroke or a selection does not touch do not render again. The
// count is the render of each row's counts, the one component every row draws
// once, so a row that re-renders shows here as a count.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceGitRow, WorkspaceGitStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitStatus: vi.fn(),
  workspaceGitDiff: vi.fn(),
  workspaceGitLog: vi.fn(),
  workspaceAgentEdits: vi.fn(),
  workspaceAgentDiff: vi.fn(),
}));

const countedRenders: string[] = [];

vi.mock("./ChangesCounts", async (importOriginal) => {
  const real = await importOriginal<typeof import("./ChangesCounts")>();
  return {
    ...real,
    ChangesCounts: (props: Parameters<typeof real.ChangesCounts>[0]) => {
      countedRenders.push(props.className ?? "");
      return real.ChangesCounts(props);
    },
  };
});

import { workspaceAgentEdits, workspaceGitDiff, workspaceGitStatus } from "../../lib/tauri";
import { ChangesSurface } from "./ChangesSurface";
import { localWorkspaceKey } from "./hosts/hostIdentity";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  countedRenders.length = 0;
  vi.mocked(workspaceGitDiff).mockResolvedValue({
    path: "src/a.ts",
    isNew: false,
    isDeleted: false,
    additions: 1,
    deletions: 0,
    lines: [{ kind: "add", text: "x" }],
    status: "ok",
    error: null,
  });
  vi.mocked(workspaceAgentEdits).mockResolvedValue({
    files: [],
    totals: { additions: 0, deletions: 0 },
    truncated: false,
    error: null,
  } as never);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

function row(path: string, additions: number): WorkspaceGitRow {
  return { path, additions, deletions: 0, status: "modified", capped: false };
}

function statusWith(rows: WorkspaceGitRow[]): WorkspaceGitStatus {
  return {
    isGit: true,
    dirty: true,
    branch: "main",
    totals: { additions: rows.length, deletions: 0 },
    rows,
    error: null,
  } as unknown as WorkspaceGitStatus;
}

const ROWS = Array.from({ length: 26 }, (_, index) =>
  row(`scout/CODER-REPORT-${index}-long-name.md`, index + 1),
);

async function renderPanel(): Promise<void> {
  vi.mocked(workspaceGitStatus).mockResolvedValue(statusWith(ROWS));
  await act(async () => {
    root.render(
      <ChangesSurface workspaceKey={localWorkspaceKey("ws-renders")!} canListCommits={false} />,
    );
  });
  await act(async () => undefined);
  countedRenders.length = 0;
}

function rowButton(path: string): HTMLButtonElement {
  const found = container.querySelector<HTMLButtonElement>(
    `.workspace-file-change[title="${path}"]`,
  );
  if (found === null) throw new Error(`row ${path} did not render`);
  return found;
}

function fileRowRenders(): number {
  return countedRenders.filter((name) => name.includes("workspace-file-change-stats")).length;
}

describe("the Changes tree's renders", () => {
  it("renders no row again for a keystroke in the commit field", async () => {
    await renderPanel();
    const input = container.querySelector<HTMLInputElement>('[aria-label="Commit message"]');
    if (input === null) throw new Error("commit field did not render");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");

    await act(async () => {
      setter.call(input, "w");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });

    // The branch total is the panel's own head and re-renders with its parent;
    // the tree's rows, folders and files alike, must not.
    expect(countedRenders.filter((name) => name !== "workspace-changes-branch-totals")).toEqual([]);
  });

  it("renders only the row whose selection changed, not the other 25", async () => {
    await renderPanel();

    await act(async () => {
      rowButton(ROWS[0]!.path).click();
    });
    expect(fileRowRenders()).toBe(1);

    countedRenders.length = 0;
    await act(async () => {
      rowButton(ROWS[1]!.path).click();
    });
    // The row given up and the row taken: two, never the whole list.
    expect(fileRowRenders()).toBe(2);
  });
});

describe("the Changes row's acts", () => {
  it("name their path in a tooltip, the overflow menu included", async () => {
    await renderPanel();
    const menu = container.querySelector<HTMLButtonElement>(
      `button[aria-label="${ROWS[0]!.path} actions"]`,
    );
    expect(menu?.getAttribute("title")).toBe(`${ROWS[0]!.path} actions`);
  });
});

describe("the Changes tree's group ids", () => {
  it("never give two folders the same id, even when a name holds hex-like text", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusWith([row("a b/x.ts", 1), row("a-20-b/y.ts", 1)]),
    );
    await act(async () => {
      root.render(
        <ChangesSurface workspaceKey={localWorkspaceKey("ws-renders")!} canListCommits={false} />,
      );
    });
    await act(async () => undefined);

    const ids = Array.from(container.querySelectorAll("ul.workspace-changes-group"), (group) =>
      group.getAttribute("id"),
    );
    expect(ids).toHaveLength(2);
    expect(new Set(ids).size).toBe(2);
  });
});
