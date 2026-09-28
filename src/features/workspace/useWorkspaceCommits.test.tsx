// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceGitCommitEntry, WorkspaceGitLog } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitLog: vi.fn(),
}));

import { workspaceGitLog } from "../../lib/tauri";
import { useWorkspaceCommits } from "./useWorkspaceCommits";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-commits-subject";

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

/** A promise the test settles itself, so a pending state is asserted, not raced. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

/**
 * The hook as a surface can read: the shas on screen, the gate, the
 * refusal's sentence, and the manual refresh the panel's own refresh
 * button and a fresh commit call.
 */
function Harness({
  workspaceId,
  open,
  canListCommits,
}: {
  workspaceId: string | null;
  open: boolean;
  canListCommits: boolean;
}) {
  const commits = useWorkspaceCommits(workspaceId, open, canListCommits);
  const shas =
    commits.log === null ? "" : commits.log.commits.map((entry) => entry.shortSha).join(",");
  return (
    <div>
      <button type="button" className="commits-refresh" onClick={() => commits.refresh()}>
        Refresh
      </button>
      <span className="commits-shas">{shas}</span>
      <span className="commits-supported">{String(commits.supported)}</span>
      <span className="commits-failure">{commits.failure?.sentence ?? ""}</span>
    </div>
  );
}

describe("useWorkspaceCommits", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceGitLog).mockResolvedValue(logReply());
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
    vi.useRealTimers();
    delete (document as { hidden?: boolean }).hidden;
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
    });
  }

  function shas(): string {
    return container.querySelector(".commits-shas")?.textContent ?? "";
  }

  function supported(): string {
    return container.querySelector(".commits-supported")?.textContent ?? "";
  }

  function failure(): string {
    return container.querySelector(".commits-failure")?.textContent ?? "";
  }

  it("reads the history the moment the view opens and renders the answer", async () => {
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={true} />);

    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledWith(WORKSPACE);
    expect(shas()).toBe("a1b2c3d");
  });

  it("reads nothing while the Commits view is closed", async () => {
    await render(<Harness workspaceId={WORKSPACE} open={false} canListCommits={true} />);

    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();
    expect(shas()).toBe("");
  });

  it("refetches every 30 seconds while the view is open", async () => {
    vi.useFakeTimers();
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={true} />);
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);

    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(2);
    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(3);
  });

  it("stops the poll the moment the view closes", async () => {
    vi.useFakeTimers();
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={true} />);
    await act(async () => {
      root.render(<Harness workspaceId={WORKSPACE} open={false} canListCommits={true} />);
    });

    await act(async () => {
      vi.advanceTimersByTime(90_000);
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);
  });

  // A hidden window keeps the interval but asks nothing: the daemon is
  // not polled for a view nobody can see — not even the first read.
  it("asks nothing while the document is hidden", async () => {
    vi.useFakeTimers();
    Object.defineProperty(document, "hidden", { value: true, configurable: true });
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={true} />);
    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();

    await act(async () => {
      vi.advanceTimersByTime(90_000);
    });
    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();

    // Back in the open, the next tick asks.
    Object.defineProperty(document, "hidden", { value: false, configurable: true });
    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);
  });

  // The recorded defect class: a reply of the previous workspace landing
  // under the new one's name. The switch happens while A's read is still
  // in flight, so both guards are exercised — the generation counter and
  // the cell's own workspace key.
  it("ignores a stale reply from the previous workspace after a switch", async () => {
    const pendingA = deferred<WorkspaceGitLog>();
    const pendingB = deferred<WorkspaceGitLog>();
    vi.mocked(workspaceGitLog).mockImplementation((workspaceId: string) =>
      workspaceId === "workspace-commits-a" ? pendingA.promise : pendingB.promise,
    );
    await render(<Harness workspaceId="workspace-commits-a" open canListCommits={true} />);
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(1);

    // Switch to B before A answers: B's read starts, A's is now the stale one.
    await act(async () => {
      root.render(<Harness workspaceId="workspace-commits-b" open canListCommits={true} />);
    });
    expect(vi.mocked(workspaceGitLog)).toHaveBeenCalledTimes(2);
    expect(shas()).toBe("");

    await act(async () => {
      pendingA.resolve(logReply({ commits: [commitEntry({ shortSha: "aaaaaaa" })] }));
    });
    expect(shas()).toBe("");
    expect(container.textContent).not.toContain("aaaaaaa");

    // B's own answer lands and is the only history on screen.
    await act(async () => {
      pendingB.resolve(logReply({ commits: [commitEntry({ shortSha: "bbbbbbb" })] }));
    });
    expect(shas()).toBe("bbbbbbb");
    expect(container.textContent).not.toContain("aaaaaaa");
  });

  // The cell's failure path: a refused read after a workspace switch
  // drops the PREVIOUS workspace's reply, not just the failure's own.
  it("drops the previous workspace's reply when a read refuses after a switch", async () => {
    vi.mocked(workspaceGitLog).mockImplementation((workspaceId: string) =>
      workspaceId === "workspace-commits-a"
        ? Promise.resolve(logReply({ commits: [commitEntry({ shortSha: "aaaaaaa" })] }))
        : Promise.reject(new Error("the daemon is restarting")),
    );
    await render(<Harness workspaceId="workspace-commits-a" open canListCommits={true} />);
    expect(shas()).toBe("aaaaaaa");

    await act(async () => {
      root.render(<Harness workspaceId="workspace-commits-b" open canListCommits={true} />);
    });

    expect(failure()).toBe("the daemon is restarting");
    expect(shas()).toBe("");
    expect(container.textContent).not.toContain("aaaaaaa");
  });

  it("keeps the last reply on screen when a later read is refused", async () => {
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={true} />);
    expect(shas()).toBe("a1b2c3d");

    vi.mocked(workspaceGitLog).mockRejectedValue(new Error("the daemon is restarting"));
    await act(async () => {
      container.querySelector<HTMLButtonElement>(".commits-refresh")?.click();
    });

    expect(failure()).toBe("the daemon is restarting");
    expect(shas()).toBe("a1b2c3d");
  });

  it("shows a refusal alone when no reply was ever read", async () => {
    vi.mocked(workspaceGitLog).mockRejectedValue(new Error("the folder is not a repository"));
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={true} />);

    expect(shas()).toBe("");
    expect(failure()).toBe("the folder is not a repository");
  });

  // Paseo's gate: the section is hidden unless the capability is present
  // (use-commits-query.ts:83-87). A connected daemon without it leaves the
  // segment hidden — and a hidden segment reads nothing, first tick or
  // thirtieth.
  it("hides the segment and reads nothing when the daemon lacks the capability", async () => {
    vi.useFakeTimers();
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={false} />);

    expect(supported()).toBe("false");
    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(90_000);
    });
    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();
  });

  it("reads nothing while the caller says the daemon cannot list history", async () => {
    await render(<Harness workspaceId={WORKSPACE} open canListCommits={false} />);

    expect(supported()).toBe("false");
    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();
  });

  it("asks nothing while no workspace is selected", async () => {
    await render(<Harness workspaceId={null} open canListCommits={true} />);

    expect(vi.mocked(workspaceGitLog)).not.toHaveBeenCalled();
  });
});
