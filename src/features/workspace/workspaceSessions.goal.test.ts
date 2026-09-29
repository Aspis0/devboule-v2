// The roster push merge keeps the snapshot's goal on the row: a present
// goal lands, and a missing or null one means no goal — never a carried one.
// A list refresh carries the row's goal forward: the wire's `Session` has
// no goal field, so an omitting list must not clear what a push landed.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { Session, SessionStateSnapshot } from "../../types/ipc";
import { createWorkspaceSessionController } from "./workspaceSessions";

function snapshot(goal: string | null | undefined): SessionStateSnapshot {
  const row: SessionStateSnapshot = {
    id: "agent-1",
    workspaceId: "w1",
    kind: "claude",
    title: "Agent",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
  return goal === undefined ? row : { ...row, goal };
}

function pushedGoal(goal: string | null | undefined): string | null | undefined {
  const mailbox: { push: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
    push: null,
  };
  const controller = createWorkspaceSessionController({
    list: async () => [],
    create: async () => {
      throw new Error("unused");
    },
    watch: async (listener) => {
      mailbox.push = listener;
      return () => undefined;
    },
  });
  const release = controller.watch();
  try {
    mailbox.push?.([snapshot(goal)]);
    return controller.getState().sessions[0]?.goal;
  } finally {
    release();
  }
}

describe("the roster push merge keeps the snapshot's goal", () => {
  it("lands a present goal on the row", () => {
    expect(pushedGoal("Move checkout to the provider registry")).toBe(
      "Move checkout to the provider registry",
    );
  });

  it("reads a missing goal as no goal", () => {
    expect(pushedGoal(undefined)).toBeNull();
  });

  it("reads a null goal as no goal", () => {
    expect(pushedGoal(null)).toBeNull();
  });

  it("clears a carried goal when the next push omits it", () => {
    const mailbox: { push: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      push: null,
    };
    const controller = createWorkspaceSessionController({
      list: async () => [],
      create: async () => {
        throw new Error("unused");
      },
      watch: async (listener) => {
        mailbox.push = listener;
        return () => undefined;
      },
    });
    const release = controller.watch();
    try {
      mailbox.push?.([snapshot("Move checkout")]);
      expect(controller.getState().sessions[0]?.goal).toBe("Move checkout");
      mailbox.push?.([snapshot(undefined)]);
      expect(controller.getState().sessions[0]?.goal ?? null).toBeNull();
    } finally {
      release();
    }
  });
});

describe("a list refresh keeps a pushed goal", () => {
  it("carries the goal across refresh, then clears it on a null push", async () => {
    let listed: Session[] = [];
    const mailbox: { push: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      push: null,
    };
    const controller = createWorkspaceSessionController({
      list: async () => listed,
      create: async () => {
        throw new Error("unused");
      },
      watch: async (listener) => {
        mailbox.push = listener;
        return () => undefined;
      },
    });
    const release = controller.watch();
    try {
      mailbox.push?.([snapshot("Move checkout")]);
      expect(controller.getState().sessions[0]?.goal).toBe("Move checkout");
      listed = [
        {
          id: "agent-1",
          workspaceId: "w1",
          kind: "claude",
          title: "Agent",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
        },
      ];
      await controller.refresh();
      expect(controller.getState().sessions[0]?.goal).toBe("Move checkout");
      mailbox.push?.([snapshot(null)]);
      expect(controller.getState().sessions[0]?.goal ?? null).toBeNull();
    } finally {
      release();
    }
  });

  it("reads an explicit null list row as no goal", async () => {
    let listed: Session[] = [];
    const mailbox: { push: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      push: null,
    };
    const controller = createWorkspaceSessionController({
      list: async () => listed,
      create: async () => {
        throw new Error("unused");
      },
      watch: async (listener) => {
        mailbox.push = listener;
        return () => undefined;
      },
    });
    const release = controller.watch();
    try {
      mailbox.push?.([snapshot("Move checkout")]);
      expect(controller.getState().sessions[0]?.goal).toBe("Move checkout");
      listed = [
        {
          id: "agent-1",
          workspaceId: "w1",
          kind: "claude",
          title: "Agent",
          state: { type: "live", generation: 1 },
          elapsedMs: 0,
          goal: null,
        },
      ];
      await controller.refresh();
      expect(controller.getState().sessions[0]?.goal ?? null).toBeNull();
    } finally {
      release();
    }
  });
});
