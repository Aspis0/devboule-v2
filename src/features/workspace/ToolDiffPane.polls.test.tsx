// @vitest-environment happy-dom

import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceGitFileDiff } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceGitDiff: vi.fn(),
}));

// Render counting through the real tab: the pane must schedule no DiffTab
// render at all when a poll answers with what is already on screen.
let diffTabRenders = 0;
vi.mock("./DiffTab", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./DiffTab")>();
  return {
    ...actual,
    DiffTab: function CountingDiffTab(props: ComponentProps<typeof actual.DiffTab>) {
      diffTabRenders += 1;
      return <actual.DiffTab {...props} />;
    },
  };
});

import { workspaceGitDiff } from "../../lib/tauri";
import { CHANGES_POLL_MS, type ChangesReply } from "./useWorkspaceChanges";
import { ToolDiffPane } from "./ToolDiffPane";
import { toolContentKey } from "./toolContentCache";
import { resetDiffTabModeMemoryForTests } from "./DiffTab";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function reply(text: string): WorkspaceGitFileDiff {
  return {
    path: "src/a.ts",
    isNew: false,
    isDeleted: false,
    additions: 1,
    deletions: 0,
    lines: [
      { kind: "header", text: "@@ -1,1 +1,2 @@" },
      { kind: "context", text: "keep" },
      { kind: "add", text },
    ],
    status: "ok",
    error: null,
  };
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("the Diff tab poll", () => {
  let container: HTMLDivElement;
  let root: Root;
  let cache: Map<string, ChangesReply<WorkspaceGitFileDiff>>;

  beforeEach(() => {
    resetDiffTabModeMemoryForTests();
    vi.useFakeTimers();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    cache = new Map();
    diffTabRenders = 0;
    vi.mocked(workspaceGitDiff).mockResolvedValue(reply("one = 1;"));
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    container.remove();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  async function mountPane() {
    await act(async () => {
      root.render(<ToolDiffPane workspaceId="ws" path="src/a.ts" refreshNonce={0} cache={cache} />);
    });
    await act(async () => {});
  }

  function paneText(): string {
    return container.textContent ?? "";
  }

  it("reads immediately and then on the Changes cadence while mounted", async () => {
    await mountPane();
    expect(vi.mocked(workspaceGitDiff)).toHaveBeenCalledTimes(1);
    expect(paneText()).toContain("one = 1;");

    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(vi.mocked(workspaceGitDiff)).toHaveBeenCalledTimes(2);

    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(vi.mocked(workspaceGitDiff)).toHaveBeenCalledTimes(3);
  });

  it("stops polling once the tab is unmounted", async () => {
    await mountPane();
    await act(async () => {
      root.unmount();
    });
    await act(async () => {
      vi.advanceTimersByTime(4 * CHANGES_POLL_MS);
    });
    expect(vi.mocked(workspaceGitDiff)).toHaveBeenCalledTimes(1);
  });

  it("renders nothing again when a poll answers with the same lines", async () => {
    await mountPane();
    expect(paneText()).toContain("one = 1;");
    const rendersAfterMount = diffTabRenders;
    expect(rendersAfterMount).toBeGreaterThan(0);

    // A new object with identical content: the reply the daemon repeats.
    vi.mocked(workspaceGitDiff).mockResolvedValue(reply("one = 1;"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(vi.mocked(workspaceGitDiff)).toHaveBeenCalledTimes(2);
    expect(diffTabRenders).toBe(rendersAfterMount);
    expect(paneText()).toContain("one = 1;");
  });

  it("shows a changed poll answer", async () => {
    await mountPane();
    expect(paneText()).toContain("one = 1;");

    vi.mocked(workspaceGitDiff).mockResolvedValue(reply("two = 2;"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(paneText()).toContain("two = 2;");
  });

  it("keeps the last good diff beside a failed poll and clears the failure on recovery", async () => {
    await mountPane();
    expect(paneText()).toContain("one = 1;");

    vi.mocked(workspaceGitDiff).mockRejectedValueOnce(new Error("daemon went away"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(paneText()).toContain("one = 1;");
    expect(paneText()).toContain("Couldn't refresh");

    vi.mocked(workspaceGitDiff).mockResolvedValue(reply("one = 1;"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(paneText()).toContain("one = 1;");
    expect(paneText()).not.toContain("Couldn't refresh");
  });

  it("drops a read overtaken by a newer one", async () => {
    const first = deferred<WorkspaceGitFileDiff>();
    const second = deferred<WorkspaceGitFileDiff>();
    vi.mocked(workspaceGitDiff)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    await mountPane();
    expect(paneText()).toContain("Loading diff");

    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    second.resolve(reply("second = 2;"));
    await act(async () => {});
    expect(paneText()).toContain("second = 2;");

    first.resolve(reply("first = 1;"));
    await act(async () => {});
    expect(paneText()).toContain("second = 2;");
    expect(paneText()).not.toContain("first = 1;");
  });

  it("schedules no render when failing polls repeat the same sentence", async () => {
    await mountPane();
    expect(paneText()).toContain("one = 1;");
    // A fresh rejection per poll: only an equal-by-value guard holds this still.
    vi.mocked(workspaceGitDiff).mockImplementation(() =>
      Promise.reject(new Error("daemon went away")),
    );
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(paneText()).toContain("Couldn't refresh");
    const rendersAfterFirstFailure = diffTabRenders;
    expect(rendersAfterFirstFailure).toBeGreaterThan(0);

    await act(async () => {
      vi.advanceTimersByTime(3 * CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(vi.mocked(workspaceGitDiff)).toHaveBeenCalledTimes(5);
    expect(diffTabRenders).toBe(rendersAfterFirstFailure);
    expect(container.querySelectorAll(".diff-tab-refresh-failure")).toHaveLength(1);
  });

  it("re-renders once when the failure reason changes", async () => {
    await mountPane();
    vi.mocked(workspaceGitDiff).mockImplementation(() =>
      Promise.reject(new Error("daemon went away")),
    );
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    const rendersAfterFirstFailure = diffTabRenders;

    vi.mocked(workspaceGitDiff).mockImplementation(() =>
      Promise.reject(new Error("the file is locked")),
    );
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(diffTabRenders).toBeGreaterThan(rendersAfterFirstFailure);
    expect(container.querySelector(".diff-tab-refresh-failure")?.textContent).toContain(
      "the file is locked",
    );
  });

  it("clears the failure line silently on recovery", async () => {
    await mountPane();
    vi.mocked(workspaceGitDiff).mockRejectedValueOnce(new Error("daemon went away"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(container.querySelector(".diff-tab-refresh-failure")).not.toBeNull();

    vi.mocked(workspaceGitDiff).mockResolvedValue(reply("one = 1;"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    // The visible line is removed, not re-announced: the live shell stays mounted but empty.
    expect(container.querySelector(".diff-tab-refresh-failure")).toBeNull();
  });

  it("never writes a failure into the content cache", async () => {
    const key = toolContentKey("ws", "src/a.ts");
    vi.mocked(workspaceGitDiff).mockRejectedValueOnce(new Error("daemon went away"));
    await mountPane();
    expect(paneText()).toContain("daemon went away");
    expect(cache.has(key)).toBe(false);

    vi.mocked(workspaceGitDiff).mockResolvedValue(reply("one = 1;"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(cache.get(key)).toEqual({ reply: reply("one = 1;"), failure: null });

    vi.mocked(workspaceGitDiff).mockRejectedValueOnce(new Error("daemon went away"));
    await act(async () => {
      vi.advanceTimersByTime(CHANGES_POLL_MS);
    });
    await act(async () => {});
    expect(paneText()).toContain("one = 1;");
    expect(cache.get(key)).toEqual({ reply: reply("one = 1;"), failure: null });
  });
});
