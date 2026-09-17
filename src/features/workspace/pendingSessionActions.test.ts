// Tests for the pure policy behind the session tab swipe: mootness,
// dismissal pruning, and crash-record verification.
import { describe, expect, it } from "vitest";
import {
  isPendingActionMoot,
  pruneDismissed,
  verifyPendingRecord,
  type PendingSessionAction,
} from "./pendingSessionActions";
import type { Session } from "../../types/ipc";

function liveSession(id: string, createdAtMs?: number): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal",
    title: id,
    state: { type: "live", generation: 1 },
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

describe("isPendingActionMoot", () => {
  it("keeps archive pending while the process runs, drops it once it ends", () => {
    expect(isPendingActionMoot({ kind: "archive" }, liveSession("s.1"))).toBe(false);
    expect(isPendingActionMoot({ kind: "archive" }, silentSession("s.1"))).toBe(false);
    expect(isPendingActionMoot({ kind: "archive" }, endedSession("s.1"))).toBe(true);
    expect(isPendingActionMoot({ kind: "archive" }, null)).toBe(true);
  });

  it("keeps delete pending no matter what the roster says", () => {
    // Absence cannot moot a delete: the strip cannot tell "ended" (which
    // still needs its close) from "destroyed" (which answers the close
    // with session_not_found), and an unreachable daemon resolves through
    // the fire path, never through silence.
    expect(isPendingActionMoot({ kind: "delete" }, liveSession("s.1"))).toBe(false);
    expect(isPendingActionMoot({ kind: "delete" }, endedSession("s.1"))).toBe(false);
    expect(isPendingActionMoot({ kind: "delete" }, null)).toBe(false);
  });
});

describe("pruneDismissed", () => {
  const dismissed = new Map<string, number | undefined>([
    ["s.1", 42],
    ["s.gone", 7],
    ["s.recycled", 7],
    ["s.pending", 9],
  ]);
  const sessions = [liveSession("s.1", 42), liveSession("s.recycled", 43)];
  const isPending = (id: string) => id === "s.pending";

  it("keeps a dismissal while its own row is still reported", () => {
    const pruned = pruneDismissed(dismissed, sessions, isPending);
    expect(pruned?.has("s.1")).toBe(true);
    expect(pruned?.has("s.pending")).toBe(true);
  });

  it("drops a confirmed row and a recycled id", () => {
    const pruned = pruneDismissed(dismissed, sessions, isPending);
    expect(pruned?.has("s.gone")).toBe(false);
    expect(pruned?.has("s.recycled")).toBe(false);
  });

  it("returns null when nothing was pruned", () => {
    expect(
      pruneDismissed(new Map([["s.1", 42]]), [liveSession("s.1", 42)], () => false),
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
