// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceAgentFileDiff, WorkspaceGitRow, WorkspaceGitStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitStatus: vi.fn(),
  workspaceGitDiff: vi.fn(),
  workspaceGitLog: vi.fn(),
  workspaceAgentEdits: vi.fn(),
  workspaceAgentDiff: vi.fn(),
}));

import {
  workspaceAgentDiff,
  workspaceAgentEdits,
  workspaceGitDiff,
  workspaceGitStatus,
} from "../../lib/tauri";
import { ChangesSurface } from "./ChangesSurface";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "ws-committed";
const KEY = localWorkspaceKey(WORKSPACE) as WorkspaceKey;

const uncommitted: WorkspaceGitRow = {
  path: "src/a.ts",
  additions: 1,
  deletions: 0,
  status: "modified",
  capped: false,
};

const statusReply = (overrides: Partial<WorkspaceGitStatus> = {}): WorkspaceGitStatus => ({
  isGit: true,
  dirty: true,
  branch: "main",
  totals: { additions: 1, deletions: 0 },
  rows: [uncommitted],
  error: null,
  ...overrides,
});

const committedDiff: WorkspaceAgentFileDiff = {
  path: "src/b.ts",
  status: "ok",
  error: null,
  lines: [{ kind: "add", text: "export const b = 1;" }],
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
  vi.mocked(workspaceGitStatus).mockResolvedValue(statusReply());
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
  });
  vi.mocked(workspaceAgentDiff).mockResolvedValue(committedDiff);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
});

async function renderWithReveal(path: string) {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <ChangesSurface workspaceKey={KEY} canListCommits={false} revealPath={{ path, nonce: 1 }} />,
    );
  });
  await act(async () => {});
  await act(async () => {});
}

describe("a revealed file that is no longer uncommitted", () => {
  it("shows the recorded edit under the caption Committed since", async () => {
    await renderWithReveal("src/b.ts");

    expect(container.querySelector(".workspace-changes-diff-meta")?.textContent).toBe(
      "Committed since",
    );
    expect(container.querySelector(".workspace-changes-diff-name")?.textContent).toBe("b.ts");
    expect(container.querySelectorAll(".workspace-changes-diff-row")).toHaveLength(1);
    expect(vi.mocked(workspaceAgentDiff)).toHaveBeenCalledWith(WORKSPACE, "src/b.ts");
  });

  it("shows a file that still has an uncommitted change as its git diff, with no caption", async () => {
    await renderWithReveal("src/a.ts");

    expect(container.querySelector(".workspace-changes-diff-meta")).toBeNull();
    expect(vi.mocked(workspaceAgentDiff)).not.toHaveBeenCalled();
  });

  it("claims no committed edit while the status list is withheld", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(
      statusReply({ rows: [], error: "the list was too large" }),
    );
    await renderWithReveal("src/b.ts");

    expect(container.querySelector(".workspace-changes-diff-meta")).toBeNull();
    expect(vi.mocked(workspaceAgentDiff)).not.toHaveBeenCalled();
  });
});
