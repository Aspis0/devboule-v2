// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceGitStatus } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({ workspaceGitStatus: vi.fn() }));

import { workspaceGitStatus } from "../../lib/tauri";
import { useHistoryBranches } from "./useHistoryBranches";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const IDS = ["ws-x"];

function Probe() {
  const branches = useHistoryBranches(IDS, true);
  return <span>{branches.get("ws-x") ?? ""}</span>;
}

function onBranch(branch: string): WorkspaceGitStatus {
  return {
    isGit: true,
    dirty: false,
    branch,
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
  };
}

async function tick(ms: number) {
  await act(async () => {
    vi.advanceTimersByTime(ms);
  });
  await act(async () => undefined);
}

describe("useHistoryBranches", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.useFakeTimers();
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.mocked(workspaceGitStatus).mockReset();
    vi.useRealTimers();
  });

  async function mount() {
    root = createRoot(container);
    await act(async () => root.render(<Probe />));
    await act(async () => undefined);
  }

  it("starts no retry while the previous read is still pending", async () => {
    const pending: Array<(status: WorkspaceGitStatus) => void> = [];
    vi.mocked(workspaceGitStatus)
      .mockRejectedValueOnce(new Error("git locked"))
      .mockImplementation(() => new Promise((resolve) => pending.push(resolve)));
    await mount();
    await tick(30_000);
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
    await tick(30_000);
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
    await act(async () => pending[0]?.(onBranch("main")));
    expect(container.textContent).toBe("main");
  });

  it("stops retrying a workspace whose reads keep failing", async () => {
    vi.mocked(workspaceGitStatus).mockRejectedValue(new Error("no such folder"));
    await mount();
    for (let attempt = 0; attempt < 10; attempt += 1) await tick(30_000);
    expect(workspaceGitStatus).toHaveBeenCalledTimes(4);
  });
});
