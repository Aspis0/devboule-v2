import { describe, expect, it } from "vitest";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import type { Session, Workspace } from "../../types/ipc";
import { LOCAL_HOST_ID, type HostId } from "./hosts/hostIdentity";
import {
  projectView,
  reconcileProjectRecords,
  workspaceView,
  type HostWorkspace,
} from "./workspaceProjects";

// What the production path stores on a failed per-project read: the cause
// through errorSentence, never a bare message (E1).
const pipeBusy = errorSentence(new Error("the pipe was busy"));

const workspace: Workspace & { hostId: HostId } = {
  id: "workspace-1",
  hostId: LOCAL_HOST_ID,
  projectId: "project-1",
  title: "devboule-v2",
  isolation: "local",
  path: "C:\\devboule",
};

const session = (over: Partial<Session> = {}): Session => ({
  id: "session-1",
  workspaceId: "workspace-1",
  kind: "claude",
  title: "agent",
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
  ...over,
});

describe("workspaceView", () => {
  it("ignores an ended session's stale attention and unattended marker", () => {
    const ended = session({
      state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
      attention: { reason: "permission", atMs: 1 },
      unattended: "yes",
    });
    // Settled, not asking: idle, with neither leaked marker.
    expect(workspaceView(workspace, [ended]).stateDot).toBe("idle");
    expect(workspaceView(workspace, [ended, session({ activity: "working" })]).stateDot).toBe(
      "pulse",
    );
  });

  it.each(["finished", "error"] as const)("does not mark %s as needing approval", (reason) => {
    expect(
      workspaceView(workspace, [session({ attention: { reason, atMs: 1 }, activity: "working" })])
        .stateDot,
    ).toBe("pulse");
  });
  it("counts only sessions actually running a turn as working", () => {
    const view = workspaceView(workspace, [
      session({ id: "running", activity: "working" }),
      session({ id: "idle", activity: "idle" }),
      session({ id: "blocked", activity: "blocked" }),
      session({ id: "unknown", activity: "unknown" }),
      session({ id: "unstated" }),
      session({ id: "shell", kind: "terminal" }),
    ]);
    // The agent rows read the same predicate: activity "working" alone runs.
    // A live terminal holds its process, so it still counts.
    expect(view.agents).toEqual({ working: 2, waiting: 0 });
    expect(view.stateDot).toBe("pulse");
  });

  it("leaves live-but-quiet agents without a working count or pulse", () => {
    const view = workspaceView(workspace, [session({ activity: "idle", elapsedMs: 5_000 })]);
    expect(view.agents).toEqual({ working: 0, waiting: 0 });
    expect(view.stateDot).toBeNull();
  });

  it("counts the workspace's working agents and the ones waiting on the user", () => {
    const waiting = session({
      id: "w1",
      state: { type: "silent", generation: 1 },
      attention: { reason: "permission", atMs: 1 },
    });
    const view = workspaceView(workspace, [
      session({ activity: "working" }),
      session({ id: "s2", activity: "working" }),
      waiting,
    ]);
    expect(view.agents).toEqual({ working: 2, waiting: 1 });
  });

  it("counts nothing for a workspace with no sessions", () => {
    expect(workspaceView(workspace, []).agents).toEqual({ working: 0, waiting: 0 });
  });

  it("carries the shortest silence across the workspace's own sessions", () => {
    // A workspace's last activity is its most recent output, so the row keeps
    // the elapsed time of the session that spoke last.
    const view = workspaceView(workspace, [
      session({ id: "quiet", elapsedMs: 900_000 }),
      session({ id: "talked", elapsedMs: 4_000 }),
      session({ id: "other", workspaceId: "workspace-2", elapsedMs: 1 }),
    ]);
    expect(view.elapsedMs).toBe(4_000);
  });

  it("carries no elapsed time when no roster row reports one", () => {
    const recovered = session({
      state: {
        type: "recovered",
        generation: 2,
        integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
      },
      elapsedMs: null,
    });
    expect(workspaceView(workspace, [recovered]).elapsedMs).toBeNull();
  });

  it("counts only the workspace's own sessions", () => {
    const view = workspaceView(workspace, [
      session({
        id: "other",
        workspaceId: "workspace-2",
        state: {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
      }),
    ]);
    expect(view.agents).toEqual({ working: 0, waiting: 0 });
  });

  it("the state dot speaks the tab chips' vocabulary, priority first", () => {
    const attention = session({ attention: { reason: "permission", atMs: 1 } });
    const unattendedSession = session({ unattended: "yes" });
    expect(workspaceView(workspace, [attention, unattendedSession]).stateDot).toBe("attention");
    expect(workspaceView(workspace, [unattendedSession]).stateDot).toBe("unattended");
    expect(workspaceView(workspace, [session({ activity: "working" })]).stateDot).toBe("pulse");
    expect(workspaceView(workspace, []).stateDot).toBeNull();
  });

  it("marks settled-only workspaces idle, and leaves silent and empty ones dotless", () => {
    const ended = session({
      state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
    });
    const recovered = session({
      state: {
        type: "recovered",
        generation: 2,
        integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
      },
    });
    const silent = session({ state: { type: "silent", generation: 1 } });
    expect(workspaceView(workspace, [ended]).stateDot).toBe("idle");
    expect(workspaceView(workspace, [recovered]).stateDot).toBe("idle");
    // A silent session still holds its process: quiet, never idle.
    expect(workspaceView(workspace, [silent]).stateDot).toBeNull();
    expect(workspaceView(workspace, []).stateDot).toBeNull();
  });
});

describe("reconcileProjectRecords", () => {
  const mk = (id: string): HostWorkspace => ({
    id,
    projectId: "project-1",
    title: id,
    hostId: LOCAL_HOST_ID,
    isolation: "local",
    path: `C:\\${id}`,
  });
  const project = (workspaces: HostWorkspace[], workspaceError?: ErrorSentence) => ({
    id: "project-1",
    name: "devboule",
    hostId: LOCAL_HOST_ID,
    path: "C:\\devboule",
    workspaces,
    workspaceError,
  });

  it("a successful reply is authoritative: removed workspaces are dropped", () => {
    const held = project([mk("kept"), mk("removed-elsewhere")]);
    const loaded = project([mk("kept")]);
    const next = reconcileProjectRecords([loaded], [held]);
    expect(next[0]!.workspaces.map((w) => w.id)).toEqual(["kept"]);
  });

  it("a failed per-project read keeps the previous records and the error", () => {
    const held = project([mk("kept"), mk("kept-2")]);
    const failed = project([], pipeBusy);
    const next = reconcileProjectRecords([failed], [held]);
    expect(next[0]!.workspaces.map((w) => w.id)).toEqual(["kept", "kept-2"]);
    expect(next[0]!.workspaceError).toEqual(pipeBusy);
  });

  it("a failed read with no previous data stays empty (nothing was held)", () => {
    const failed = project([], pipeBusy);
    const next = reconcileProjectRecords([failed], []);
    expect(next[0]!.workspaces).toEqual([]);
    expect(next[0]!.workspaceError).toEqual(pipeBusy);
  });
});

describe("projectView", () => {
  it("numbers rows that share a title so each row reads apart", () => {
    const sameTitle = (id: string): HostWorkspace => ({
      id,
      projectId: "project-1",
      title: "devboule-v2",
      hostId: LOCAL_HOST_ID,
      isolation: "local",
      path: "C:\\devboule-v2",
    });
    const view = projectView(
      {
        id: "project-1",
        name: "devboule-v2",
        hostId: LOCAL_HOST_ID,
        path: "C:\\devboule-v2",
        workspaces: [sameTitle("w1"), sameTitle("w2")],
      },
      new Map(),
    );
    expect(view.workspaces.map((workspace) => workspace.displayTitle)).toEqual([
      "devboule-v2",
      "devboule-v2 2",
    ]);
  });
});
