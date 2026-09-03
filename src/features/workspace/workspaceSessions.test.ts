import { describe, expect, it, vi } from "vitest";
import type { Session, SessionStateSnapshot } from "../../types/ipc";
import { createWorkspaceSessionController, sessionStateLabel } from "./workspaceSessions";

const liveSession = (id: string, title = id): Session => ({
  id,
  workspaceId: null,
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

describe("workspace session controller", () => {
  it("loads only terminal sessions and selects the first real session", async () => {
    const list = vi.fn(async () => [
      liveSession("terminal-1", "shell one"),
      {
        ...liveSession("agent-1", "agent"),
        kind: "acp" as const,
      },
      liveSession("terminal-2", "shell two"),
    ]);
    const create = vi.fn(async () => liveSession("terminal-3", "shell three"));
    const controller = createWorkspaceSessionController({ list, create });

    await controller.refresh();

    expect(controller.getState()).toMatchObject({
      sessions: [liveSession("terminal-1", "shell one"), liveSession("terminal-2", "shell two")],
      selectedSessionId: "terminal-1",
      error: null,
    });
  });

  it("adds the created terminal and selects it without inventing session data", async () => {
    const created = liveSession("terminal-3", "shell three");
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1")]),
      create: vi.fn(async () => created),
    });
    await controller.refresh();

    await controller.create();

    expect(controller.getState().sessions).toEqual([liveSession("terminal-1"), created]);
    expect(controller.getState().selectedSessionId).toBe("terminal-3");
  });

  it("keeps a visible error when the daemon cannot list sessions", async () => {
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => {
        throw new Error("daemon unreachable");
      }),
      create: vi.fn(async () => liveSession("terminal-1")),
    });

    await controller.refresh();

    expect(controller.getState()).toMatchObject({
      sessions: [],
      selectedSessionId: null,
      error: "Could not load terminal sessions. The daemon is unreachable.",
    });
  });

  it("shows observed silence with its elapsed age instead of calling it idle", () => {
    expect(sessionStateLabel({ type: "silent", generation: 1 }, 40 * 60 * 1000)).toBe(
      "silent · 40 minutes",
    );
    expect(sessionStateLabel({ type: "silent", generation: 1 })).toBe("silent · duration unknown");
  });

  it("labels a finished session with an uncertified transcript", () => {
    const state = {
      type: "ended" as const,
      generation: 1,
      code: 1,
      integrity: {
        kind: "truncated" as const,
        droppedFrames: 2,
        droppedBytes: 12 * 1024,
        trimmedBytes: 0,
      },
    };
    expect(sessionStateLabel(state)).toBe("ended · truncated");
  });

  it("updates the tab roster from a pushed session snapshot", async () => {
    const watched: {
      listener: ((snapshots: SessionStateSnapshot[]) => void) | null;
    } = { listener: null };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1", "old title")]),
      create: vi.fn(async () => liveSession("terminal-2")),
      watch: vi.fn(async (listener) => {
        watched.listener = listener;
        return () => {
          watched.listener = null;
        };
      }),
    });

    const release = controller.watch();
    await controller.refresh();
    watched.listener?.([
      {
        id: "terminal-1",
        title: "killed shell",
        state: { type: "ended", generation: 1, code: 137, integrity: { kind: "complete" } },
        elapsedMs: 42,
      },
    ]);

    expect(controller.getState().sessions).toEqual([
      {
        ...liveSession("terminal-1", "old title"),
        title: "killed shell",
        state: { type: "ended", generation: 1, code: 137, integrity: { kind: "complete" } },
        elapsedMs: 42,
      },
    ]);
    release();
  });
});
