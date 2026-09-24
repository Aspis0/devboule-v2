import { describe, expect, it } from "vitest";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import type { Session, Workspace } from "../../types/ipc";
import { reconcileProjectRecords, workspaceView } from "./workspaceProjects";

// What the production path stores on a failed per-project read: the cause
// through errorSentence, never a bare message (E1).
const pipeBusy = errorSentence(new Error("the pipe was busy"));

const workspace: Workspace = {
  id: "workspace-1",
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
  it("shows no meta line for a row that matches the norm", () => {
    const view = workspaceView(workspace, [session()]);
    expect(view.meta).toBeNull();
  });

  it("speaks only the anomaly: recovered sessions count", () => {
    const view = workspaceView(workspace, [
      session({
        id: "r1",
        state: {
          type: "recovered",
          generation: 2,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
      }),
      session({
        id: "r2",
        state: {
          type: "recovered",
          generation: 2,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        },
      }),
      session(),
    ]);
    expect(view.meta).toBe("2 recovered");
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
    expect(view.meta).toBeNull();
  });

  it("the state dot speaks the tab chips' vocabulary, priority first", () => {
    const attention = session({ attention: { reason: "permission", atMs: 1 } });
    const unattendedSession = session({ unattended: "yes" });
    expect(workspaceView(workspace, [attention, unattendedSession]).stateDot).toBe("attention");
    expect(workspaceView(workspace, [unattendedSession]).stateDot).toBe("unattended");
    expect(workspaceView(workspace, [session()]).stateDot).toBe("pulse");
    expect(workspaceView(workspace, []).stateDot).toBeNull();
  });
});

describe("reconcileProjectRecords", () => {
  const mk = (id: string): Workspace => ({
    id,
    projectId: "project-1",
    title: id,
    isolation: "local",
    path: `C:\\${id}`,
  });
  const project = (workspaces: Workspace[], workspaceError?: ErrorSentence) => ({
    id: "project-1",
    name: "devboule",
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
