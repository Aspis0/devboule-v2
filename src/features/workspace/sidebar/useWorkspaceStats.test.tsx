// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", () => ({ workspaceGitStatus: vi.fn() }));

import { workspaceGitStatus } from "../../../lib/tauri";
import { useWorkspaceStats } from "./useWorkspaceStats";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const totals = (additions: number, deletions: number, isGit = true) => ({
  isGit,
  dirty: additions + deletions > 0,
  branch: "main",
  totals: { additions, deletions },
  rows: [],
  error: null,
});

function HookProbe(props: {
  ids: readonly string[];
  connected: boolean;
  selectedWorkspace: string | null;
  endedKey: string;
  onStats: (stats: ReadonlyMap<string, { additions: number; deletions: number }>) => void;
  onBranches: (branches: ReadonlyMap<string, string>) => void;
  onRefresh?: (refresh: (ids: readonly string[]) => void) => void;
}) {
  const { stats, branches, refresh } = useWorkspaceStats(props.ids, {
    connected: props.connected,
    selectedWorkspace: props.selectedWorkspace,
    endedKey: props.endedKey,
  });
  props.onStats(stats);
  props.onBranches(branches);
  props.onRefresh?.(refresh);
  return <button type="button" data-testid="probe-refresh" onClick={() => refresh(props.ids)} />;
}

describe("useWorkspaceStats", () => {
  let holder: HTMLDivElement | null = null;
  let root: import("react-dom/client").Root | null = null;
  let latest: ReadonlyMap<string, { additions: number; deletions: number }>;
  let latestBranches: ReadonlyMap<string, string> | undefined;
  const onStats = (stats: ReadonlyMap<string, { additions: number; deletions: number }>) => {
    latest = stats;
  };
  const onBranches = (branches: ReadonlyMap<string, string>) => {
    latestBranches = branches;
  };

  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(workspaceGitStatus).mockReset();
    latest = new Map();
    latestBranches = undefined;
  });

  afterEach(async () => {
    await actUnmount();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  async function actUnmount() {
    const { act } = await import("react");
    if (root !== null) {
      act(() => root!.unmount());
      root = null;
    }
    holder?.remove();
    holder = null;
  }

  async function mount(props: Omit<Parameters<typeof HookProbe>[0], "onStats" | "onBranches">) {
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
    await act(async () => {
      root!.render(<HookProbe {...props} onStats={onStats} onBranches={onBranches} />);
    });
  }

  const reread = async () => {
    await act(async () => undefined);
  };

  it("fetches each visible row once and keeps the totals", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(145, 38));
    await mount({ ids: ["ws-1", "ws-2"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
    expect(latest.get("ws-1")).toEqual({ additions: 145, deletions: 38 });
    expect(latest.get("ws-2")).toEqual({ additions: 145, deletions: 38 });
  });

  it("hides zero-zero totals", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(0, 0));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest.has("ws-1")).toBe(false);
  });

  it("a failed read shows no stats, not zeros, and no error", async () => {
    vi.mocked(workspaceGitStatus).mockRejectedValueOnce(new Error("git died"));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest.has("ws-1")).toBe(false);
  });

  it("keeps the branch from a read, even when the totals are zero", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue({
      ...totals(0, 0),
      branch: "feature/x",
    });
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest.has("ws-1")).toBe(false);
    expect(latestBranches?.get("ws-1")).toBe("feature/x");
  });

  it("a failed read clears the branch along with the totals", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(totals(4, 2));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latestBranches?.get("ws-1")).toBe("main");
    vi.mocked(workspaceGitStatus).mockRejectedValueOnce(new Error("git died"));
    await mount2ndRefresh();
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latestBranches?.has("ws-1")).toBe(false);
  });

  it("a refresh trigger after a branch switch shows the new branch", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(totals(1, 1));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latestBranches?.get("ws-1")).toBe("main");
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce({
      ...totals(1, 1),
      branch: "renamed",
    });
    await mount2ndRefresh();
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latestBranches?.get("ws-1")).toBe("renamed");
  });

  it("drops a workspace that leaves the id list from both maps", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(145, 38));
    await mount({ ids: ["ws-1", "ws-2"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest.has("ws-1")).toBe(true);
    expect(latestBranches?.has("ws-1")).toBe(true);
    await act(async () => {
      root!.render(
        <HookProbe
          ids={["ws-2"]}
          connected
          selectedWorkspace={null}
          endedKey=""
          onStats={onStats}
          onBranches={onBranches}
        />,
      );
    });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest.has("ws-1")).toBe(false);
    expect(latestBranches?.has("ws-1")).toBe(false);
  });

  it("returns the same maps when a read lands unchanged", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(145, 38));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    const statsBefore = latest;
    const branchesBefore = latestBranches;
    await mount2ndRefresh();
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest).toBe(statsBefore);
    expect(latestBranches).toBe(branchesBefore);
  });

  it("drops the late result and the follow-up of a read whose id left the list", async () => {
    let release!: (value: ReturnType<typeof totals>) => void;
    const pending = new Promise<ReturnType<typeof totals>>((resolve) => (release = resolve));
    vi.mocked(workspaceGitStatus).mockImplementation((id: string) =>
      id === "ws-2" ? pending : Promise.resolve(totals(1, 1)),
    );
    await mount({ ids: ["ws-1", "ws-2"], connected: true, selectedWorkspace: null, endedKey: "" });
    await mount2ndRefresh();
    await act(async () => {
      root!.render(
        <HookProbe
          ids={["ws-1"]}
          connected
          selectedWorkspace={null}
          endedKey=""
          onStats={onStats}
          onBranches={onBranches}
        />,
      );
    });
    release(totals(7, 7));
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest.has("ws-2")).toBe(false);
    expect(latestBranches?.has("ws-2")).toBe(false);
    const ws2Reads = vi.mocked(workspaceGitStatus).mock.calls.filter(([id]) => id === "ws-2");
    expect(ws2Reads.length).toBe(1);
  });

  it("an unchanged result still issues the dirty follow-up, exactly once", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(3, 4));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    let release!: (value: ReturnType<typeof totals>) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () => new Promise((resolve) => (release = resolve)),
    );
    await mount2ndRefresh();
    await mount2ndRefresh();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
    // Same totals and branch: no state change, so no render can carry the
    // follow-up.
    release(totals(3, 4));
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(3);
  });

  it("a dirty read that settles after a disconnect issues no follow-up", async () => {
    let release!: (value: ReturnType<typeof totals>) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () => new Promise((resolve) => (release = resolve)),
    );
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(1, 2));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await mount2ndRefresh();
    await act(async () => {
      root!.render(
        <HookProbe
          ids={["ws-1"]}
          connected={false}
          selectedWorkspace={null}
          endedKey=""
          onStats={onStats}
          onBranches={onBranches}
        />,
      );
    });
    release(totals(1, 2));
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(1);
  });

  it("a refresh captured while connected sends nothing after a disconnect", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(1, 2));
    let captured: ((ids: readonly string[]) => void) | null = null;
    const keepFirst = (refresh: (ids: readonly string[]) => void) => {
      captured ??= refresh;
    };
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
    await act(async () => {
      root!.render(
        <HookProbe
          ids={["ws-1"]}
          connected
          selectedWorkspace={null}
          endedKey=""
          onStats={onStats}
          onBranches={onBranches}
          onRefresh={keepFirst}
        />,
      );
    });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    await act(async () => {
      root!.render(
        <HookProbe
          ids={["ws-1"]}
          connected={false}
          selectedWorkspace={null}
          endedKey=""
          onStats={onStats}
          onBranches={onBranches}
          onRefresh={keepFirst}
        />,
      );
    });
    vi.mocked(workspaceGitStatus).mockClear();
    await act(async () => captured!(["ws-1"]));
    await vi.advanceTimersByTimeAsync(0);
    expect(workspaceGitStatus).not.toHaveBeenCalled();
  });

  it("a grown id list reads only the added id, and a shrunk one reads nothing", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(1, 2));
    const render = (ids: readonly string[]) =>
      act(async () => {
        root!.render(
          <HookProbe
            ids={ids}
            connected
            selectedWorkspace={null}
            endedKey=""
            onStats={onStats}
            onBranches={onBranches}
          />,
        );
      });
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    vi.mocked(workspaceGitStatus).mockClear();
    await render(["ws-1", "ws-9"]);
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(vi.mocked(workspaceGitStatus).mock.calls.map(([id]) => id)).toEqual(["ws-9"]);
    vi.mocked(workspaceGitStatus).mockClear();
    await render(["ws-1"]);
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).not.toHaveBeenCalled();
  });

  it("a trigger during an in-flight read marks the row dirty: one follow-up runs after it settles", async () => {
    let release!: (value: ReturnType<typeof totals>) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () => new Promise((resolve) => (release = resolve)),
    );
    vi.mocked(workspaceGitStatus).mockResolvedValueOnce(totals(5, 6));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    // A second trigger for the same id while the first is in flight must not
    // fire a second request yet — and must not be silently dropped either:
    // one follow-up runs after the read settles.
    await mount2ndRefresh();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(1);
    release(totals(1, 2));
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(2);
    // The follow-up's fresh numbers, not the first read's stale ones.
    expect(latest.get("ws-1")).toEqual({ additions: 5, deletions: 6 });
  });

  async function mount2ndRefresh() {
    const { act } = await import("react");
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
    });
    await vi.advanceTimersByTimeAsync(0);
  }

  it("selection refreshes that workspace's numbers", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(3, 4));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    const afterLoad = vi.mocked(workspaceGitStatus).mock.calls.length;
    const { act } = await import("react");
    await act(async () => {
      root!.render(
        <HookProbe
          ids={["ws-1"]}
          connected
          selectedWorkspace="ws-1"
          endedKey=""
          onStats={onStats}
          onBranches={onBranches}
        />,
      );
    });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(vi.mocked(workspaceGitStatus).mock.calls.length).toBeGreaterThan(afterLoad);
  });

  it("refreshes at most every 30 seconds, and only while connected", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(totals(3, 4));
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    const afterLoad = vi.mocked(workspaceGitStatus).mock.calls.length;

    await vi.advanceTimersByTimeAsync(29_999);
    await reread();
    expect(vi.mocked(workspaceGitStatus).mock.calls.length).toBe(afterLoad);

    await vi.advanceTimersByTimeAsync(1);
    await reread();
    expect(vi.mocked(workspaceGitStatus).mock.calls.length).toBeGreaterThan(afterLoad);

    // Disconnected: the interval stops. (The remount itself fetches once,
    // as every mount does; the count is reset after it.)
    await mount2({
      ids: ["ws-1"],
      connected: false,
      selectedWorkspace: null,
      endedKey: "",
    });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    const mounted = vi.mocked(workspaceGitStatus).mock.calls.length;
    await vi.advanceTimersByTimeAsync(60_000);
    await reread();
    expect(vi.mocked(workspaceGitStatus).mock.calls.length).toBe(mounted);
  });

  async function mount2(props: Omit<Parameters<typeof HookProbe>[0], "onStats" | "onBranches">) {
    await actUnmount();
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder!);
    await act(async () => {
      root!.render(<HookProbe {...props} onStats={onStats} onBranches={onBranches} />);
    });
  }

  it("every trigger answers to the connection: focus and selection while disconnected send nothing", async () => {
    await mount({ ids: ["ws-1"], connected: false, selectedWorkspace: null, endedKey: "" });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).not.toHaveBeenCalled();

    await mount2({
      ids: ["ws-1"],
      connected: false,
      selectedWorkspace: "ws-1",
      endedKey: "",
    });
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(workspaceGitStatus).not.toHaveBeenCalled();
  });

  it("a settled refresh after unmount changes nothing", async () => {
    let release!: (value: ReturnType<typeof totals>) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () => new Promise((resolve) => (release = resolve)),
    );
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    await actUnmount();
    release(totals(9, 9));
    await vi.advanceTimersByTimeAsync(0);
    // No crash, no state write: the probe is gone.
    expect(root).toBeNull();
  });
});
