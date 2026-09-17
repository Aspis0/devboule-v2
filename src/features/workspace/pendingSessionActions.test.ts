// Tests for the pure policy behind the session tab swipe: mootness,
// dismissal pruning, and crash-record verification.
import { describe, expect, it } from "vitest";
import {
  pendingFate,
  pruneDismissed,
  verifyPendingRecord,
  type PendingSessionAction,
} from "./pendingSessionActions";
import type { Session } from "../../types/ipc";

function liveSession(id: string, createdAtMs?: number, generation = 1): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal",
    title: id,
    state: { type: "live", generation },
    elapsedMs: 0,
    ...(createdAtMs === undefined ? {} : { createdAtMs }),
  };
}

function endedSession(id: string): Session {
  return {
    ...liveSession(id),
    state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
  };
}

function silentSession(id: string): Session {
  return { ...liveSession(id), state: { type: "silent", generation: 1 } };
}

describe("pendingFate", () => {
  it("keeps archive pending while the same instance runs, hides it once it ends", () => {
    const intent = { kind: "archive", generation: 1 } as const;
    expect(pendingFate(intent, liveSession("s.1"))).toBe("keep");
    expect(pendingFate(intent, silentSession("s.1"))).toBe("keep");
    expect(pendingFate(intent, endedSession("s.1"))).toBe("void-hidden");
    expect(pendingFate(intent, null)).toBe("void-hidden");
  });

  it("voids visibly when the row comes back as a new instance", () => {
    // A resume keeps id and stamp and bumps generation: the old intent
    // must not fire at the new process, and the tab must show.
    const resumed = liveSession("s.1", 42, 2);
    expect(pendingFate({ kind: "archive", generation: 1 }, resumed)).toBe("void-visible");
    expect(pendingFate({ kind: "delete", generation: 1 }, resumed)).toBe("void-visible");
    // Same instance still stands.
    expect(pendingFate({ kind: "archive", generation: 2 }, resumed)).toBe("keep");
  });

  it("keeps delete pending no matter what the roster says, short of a resume", () => {
    // Absence cannot moot a delete: the strip cannot tell "ended" (which
    // still needs its close) from "destroyed" (which answers the close
    // with session_not_found), and an unreachable daemon resolves through
    // the fire path, never through silence.
    const intent = { kind: "delete", generation: 1 } as const;
    expect(pendingFate(intent, liveSession("s.1"))).toBe("keep");
    expect(pendingFate(intent, endedSession("s.1"))).toBe("keep");
    expect(pendingFate(intent, null)).toBe("keep");
  });
});

describe("pruneDismissed", () => {
  const dismissed = new Map([
    ["s.1", { createdAtMs: 42, generation: 1 }],
    ["s.gone", { createdAtMs: 7, generation: 1 }],
    ["s.recycled", { createdAtMs: 7, generation: 1 }],
    ["s.resumed", { createdAtMs: 42, generation: 1 }],
    ["s.pending", { createdAtMs: 9, generation: 1 }],
  ]);
  const sessions = [
    liveSession("s.1", 42),
    liveSession("s.recycled", 43),
    liveSession("s.resumed", 42, 2),
  ];
  const isPending = (id: string) => id === "s.pending";

  it("keeps a dismissal while its own instance is still reported", () => {
    const pruned = pruneDismissed(dismissed, sessions, isPending);
    expect(pruned?.has("s.1")).toBe(true);
    expect(pruned?.has("s.pending")).toBe(true);
  });

  it("drops a confirmed row, a recycled id, and a resumed instance", () => {
    const pruned = pruneDismissed(dismissed, sessions, isPending);
    expect(pruned?.has("s.gone")).toBe(false);
    expect(pruned?.has("s.recycled")).toBe(false);
    // Same stamp, new generation: the reopened session must show.
    expect(pruned?.has("s.resumed")).toBe(false);
  });

  it("returns null when nothing was pruned", () => {
    expect(
      pruneDismissed(
        new Map([["s.1", { createdAtMs: 42, generation: 1 }]]),
        [liveSession("s.1", 42)],
        () => false,
      ),
    ).toBeNull();
    expect(pruneDismissed(new Map(), sessions, isPending)).toBeNull();
  });
});

describe("verifyPendingRecord", () => {
  const record: PendingSessionAction = {
    id: "s.1",
    title: "shell",
    kind: "delete",
    createdAtMs: 42,
    generation: 1,
    dueAt: 1_000_000,
  };

  it("verifies the row when the creation time still matches", () => {
    expect(verifyPendingRecord(record, [liveSession("s.1", 42)])?.id).toBe("s.1");
  });

  it("drops the record when the id was recycled by a daemon restart", () => {
    expect(verifyPendingRecord(record, [liveSession("s.1", 43)])).toBeNull();
    expect(verifyPendingRecord(record, [])).toBeNull();
  });

  it("drops the record when either side cannot prove sameness", () => {
    const withoutStamp: PendingSessionAction = { ...record, createdAtMs: undefined };
    expect(verifyPendingRecord(withoutStamp, [liveSession("s.1", 42)])).toBeNull();
    expect(verifyPendingRecord(record, [liveSession("s.1")])).toBeNull();
  });
});
