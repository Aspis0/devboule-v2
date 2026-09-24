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
}) {
  const stats = useWorkspaceStats(props.ids, {
    connected: props.connected,
    selectedWorkspace: props.selectedWorkspace,
    endedKey: props.endedKey,
  });
  props.onStats(stats);
  return null;
}

describe("useWorkspaceStats", () => {
  let holder: HTMLDivElement | null = null;
  let root: import("react-dom/client").Root | null = null;
  let latest: ReadonlyMap<string, { additions: number; deletions: number }>;
  const onStats = (stats: ReadonlyMap<string, { additions: number; deletions: number }>) => {
    latest = stats;
  };

  beforeEach(() => {
    vi.useFakeTimers();
    vi.mocked(workspaceGitStatus).mockReset();
    latest = new Map();
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

  async function mount(props: Omit<Parameters<typeof HookProbe>[0], "onStats">) {
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
    await act(async () => {
      root!.render(<HookProbe {...props} onStats={onStats} />);
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

  it("runs one request in flight per workspace: an overlapping refresh is skipped", async () => {
    let release!: (value: ReturnType<typeof totals>) => void;
    vi.mocked(workspaceGitStatus).mockImplementationOnce(
      () => new Promise((resolve) => (release = resolve)),
    );
    await mount({ ids: ["ws-1"], connected: true, selectedWorkspace: null, endedKey: "" });
    // A second trigger for the same id while the first is in flight.
    await mount2ndRefresh();
    expect(workspaceGitStatus).toHaveBeenCalledTimes(1);
    release(totals(1, 2));
    await vi.advanceTimersByTimeAsync(0);
    await reread();
    expect(latest.get("ws-1")).toEqual({ additions: 1, deletions: 2 });
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

  async function mount2(props: Omit<Parameters<typeof HookProbe>[0], "onStats">) {
    await actUnmount();
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder!);
    await act(async () => {
      root!.render(<HookProbe {...props} onStats={onStats} />);
    });
  }

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
