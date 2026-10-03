// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { workspaceGitStatus } from "../../lib/tauri";
import type { WorkspaceGitStatus } from "../../types/ipc";
import { rememberChangesStatus } from "./changesStatusCache";
import { useWorkspaceChanges } from "./useWorkspaceChanges";
import { useMenuBranch } from "./useMenuBranch";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

vi.mock("../../lib/tauri", () => ({ workspaceGitStatus: vi.fn(), workspaceGitDiff: vi.fn() }));
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const status = (branch: string | null = "feature/own"): WorkspaceGitStatus => ({
  isGit: true,
  branch,
  dirty: false,
  totals: { additions: 0, deletions: 0 },
  rows: [],
  error: null,
});
let root: Root;
let host: HTMLDivElement;
function Probe({
  workspaceKey = keyFor("own"),
  open = true,
}: {
  workspaceKey?: WorkspaceKey | null;
  open?: boolean;
}) {
  return <span>{useMenuBranch(workspaceKey, open)}</span>;
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(workspaceGitStatus).mockReset().mockResolvedValue(status());
  rememberChangesStatus(keyFor("own"), null);
  rememberChangesStatus(keyFor("other"), null);
  host = document.createElement("div");
  root = createRoot(host);
});
afterEach(async () => {
  await act(async () => root.unmount());
  vi.useRealTimers();
});
const render = async (workspaceId: string | null = "own", open = true) => {
  await act(async () =>
    root.render(
      <Probe workspaceKey={workspaceId === null ? null : keyFor(workspaceId)} open={open} />,
    ),
  );
};

describe("menu branch reads", () => {
  it("reads once on open and does not poll", async () => {
    await render("own", false);
    expect(workspaceGitStatus).not.toHaveBeenCalled();
    await render();
    expect(host.textContent).toBe("feature/own");
    await render();
    expect(workspaceGitStatus).toHaveBeenCalledExactlyOnceWith("own");
    await act(async () => vi.advanceTimersByTimeAsync(20000));
    expect(workspaceGitStatus).toHaveBeenCalledExactlyOnceWith("own");
    await render("own", false);
    await render();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
  });

  it.each([null, "", "   ", "(detached)", "HEAD"])("hides unusable branch %s", async (branch) => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(status(branch));
    await render();
    expect(host.textContent).toBe("");
  });

  it.each(["non-git", "status error", "rejection", "no workspace"])("hides %s", async (kind) => {
    if (kind === "non-git")
      vi.mocked(workspaceGitStatus).mockResolvedValue({ ...status(), isGit: false });
    if (kind === "status error")
      vi.mocked(workspaceGitStatus).mockResolvedValue({ ...status(), error: "unavailable" });
    if (kind === "rejection")
      vi.mocked(workspaceGitStatus).mockRejectedValue(new Error("unavailable"));
    await render(kind === "no workspace" ? null : "own");
    expect(host.textContent).toBe("");
  });

  it.each([0, 5000, 5001])("reuses Changes data only while fresh at %s ms", async (age) => {
    rememberChangesStatus(keyFor("own"), status("cached"));
    await act(async () => vi.advanceTimersByTimeAsync(age));
    await render();
    expect(host.textContent).toBe(age <= 5000 ? "cached" : "feature/own");
    expect(workspaceGitStatus).toHaveBeenCalledTimes(age <= 5000 ? 0 : 1);
  });

  it("invalidates cached branch after a Changes error", async () => {
    rememberChangesStatus(keyFor("own"), status("cached"));
    rememberChangesStatus(keyFor("own"), null);
    await render();
    expect(workspaceGitStatus).toHaveBeenCalledExactlyOnceWith("own");
  });

  it("ignores late replies after close and reopening", async () => {
    let complete!: (value: WorkspaceGitStatus) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    await render();
    expect(host.textContent).toBe("");
    await render("own", false);
    await act(async () => complete(status("late")));
    expect(host.textContent).toBe("");
    await render();
    expect(host.textContent).toBe("feature/own");
  });

  it("ignores a reply for the previous workspace", async () => {
    let complete!: (value: WorkspaceGitStatus) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    await render();
    await render("other");
    await act(async () => complete(status("stale")));
    expect(host.textContent).toBe("feature/own");
    expect(vi.mocked(workspaceGitStatus).mock.calls).toEqual([["own"], ["other"]]);
  });
  it("hides a prior opening's branch while a new opening is loading", async () => {
    await render();
    expect(host.textContent).toBe("feature/own");
    await render("own", false);
    let complete!: (value: WorkspaceGitStatus) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    await render();
    expect(host.textContent).toBe("");
    await act(async () => complete(status("new-branch")));
    expect(host.textContent).toBe("new-branch");
  });
  it("reuses status actually read by the Changes hook", async () => {
    function ChangesProbe() {
      useWorkspaceChanges(keyFor("own"));
      return null;
    }
    await act(async () => root.render(<ChangesProbe />));
    expect(workspaceGitStatus).toHaveBeenCalledExactlyOnceWith("own");
    await render();
    expect(host.textContent).toBe("feature/own");
    expect(workspaceGitStatus).toHaveBeenCalledTimes(1);
  });

  it("clears cached Changes status when the wired status read rejects", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(status("stale"));
    function ChangesProbe() {
      const changes = useWorkspaceChanges(keyFor("own"));
      return <button onClick={changes.refresh}>Refresh</button>;
    }
    await act(async () => root.render(<ChangesProbe />));
    expect(workspaceGitStatus).toHaveBeenCalledExactlyOnceWith("own");
    vi.mocked(workspaceGitStatus).mockRejectedValueOnce(new Error("offline"));
    await act(async () => host.querySelector("button")!.click());
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
    await render();
    expect(host.textContent).toBe("feature/own");
    expect(workspaceGitStatus).toHaveBeenCalledTimes(3);
  });

  it("keeps a Changes reply after cleanup out of the shared cache", async () => {
    let complete!: (value: WorkspaceGitStatus) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    );
    function ChangesProbe() {
      useWorkspaceChanges(keyFor("own"));
      return null;
    }
    await act(async () => root.render(<ChangesProbe />));
    expect(workspaceGitStatus).toHaveBeenCalledExactlyOnceWith("own");
    await act(async () => root.render(<Probe open={false} />));
    await act(async () => complete(status("late-changes")));
    await render();
    expect(host.textContent).toBe("feature/own");
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
  });
});
