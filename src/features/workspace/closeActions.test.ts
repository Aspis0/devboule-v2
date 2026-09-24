// The close-act store: firing hides the row at once; a failure brings it
// back with the reason, owned by the act that produced it; a later clean
// close — or a session_not_found, which is the same thing arrived at by
// another hand — clears the session's line; and an older build's persisted
// undo records are startup litter, dropped unread.

import { describe, expect, it, vi } from "vitest";
import {
  CloseActionStore,
  OLDER_BUILD_PENDING_KEY,
  discardPersistedPendingCloses,
  type CloseTarget,
} from "./closeActions";

function target(id: string, generation = 1): CloseTarget {
  return { id, title: id, generation };
}

function liveRow(id: string, generation = 1) {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal" as const,
    title: id,
    state: { type: "live" as const, generation },
    elapsedMs: 0,
  };
}

async function settleStore(store: CloseActionStore): Promise<void> {
  await vi.advanceTimersByTimeAsync(0);
  // Touch nothing: the snapshots are read through the getters below.
  void store;
}

describe("CloseActionStore", () => {
  it("hides the row at once and fires the daemon call immediately", async () => {
    vi.useFakeTimers();
    const archive = vi.fn(async () => undefined);
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });
    expect(store.getClosingSnapshot()).toEqual([]);

    store.act("archive", target("s.1"));

    expect(archive).toHaveBeenCalledWith("s.1");
    expect(store.getClosingSnapshot()).toEqual(["s.1"]);
    await settleStore(store);
    expect(store.getClosingSnapshot()).toEqual(["s.1"]);
    vi.useRealTimers();
  });

  it("brings the row back and names the failure when the daemon refuses", async () => {
    vi.useFakeTimers();
    const archive = vi.fn(async () => {
      throw new Error("daemon refused stop");
    });
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });

    store.act("archive", target("s.1"));
    await settleStore(store);

    expect(store.getClosingSnapshot()).toEqual([]);
    expect(store.getFailuresSnapshot()).toEqual([
      { id: "s.1", message: "Archive of “s.1” failed: daemon refused stop" },
    ]);
    vi.useRealTimers();
  });

  it("a later clean close of the same session clears its earlier failure", async () => {
    vi.useFakeTimers();
    const archive = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error("daemon refused stop"))
      .mockResolvedValueOnce(undefined);
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });

    store.act("archive", target("s.1"));
    await settleStore(store);
    expect(store.getFailuresSnapshot()).toHaveLength(1);

    // The failure's row came back; the user closes it again, cleanly.
    store.act("archive", target("s.1"));
    await settleStore(store);
    expect(store.getFailuresSnapshot()).toEqual([]);
    vi.useRealTimers();
  });

  it("session_not_found clears the failure and leaves the row hidden", async () => {
    vi.useFakeTimers();
    // First close fails; the retry answers session_not_found — the session
    // is gone by another hand, which is the postcondition arrived at late.
    const archive = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error("daemon refused stop"))
      .mockRejectedValueOnce({ code: "session_not_found", message: "no such session" });
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });

    store.act("archive", target("s.1"));
    await settleStore(store);
    expect(store.getFailuresSnapshot()).toHaveLength(1);

    store.act("archive", target("s.1"));
    await settleStore(store);

    expect(store.getFailuresSnapshot()).toEqual([]);
    expect(store.getClosingSnapshot()).toEqual(["s.1"]);
    vi.useRealTimers();
  });

  it("a stale target is skipped and reported, never acted on", async () => {
    vi.useFakeTimers();
    const archive = vi.fn(async () => undefined);
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });

    store.skipped("archive", target("s.1"));

    expect(archive).not.toHaveBeenCalled();
    expect(store.getFailuresSnapshot()).toEqual([
      {
        id: "s.1",
        message: "Archive of “s.1” skipped — the session changed after it was confirmed.",
      },
    ]);
    vi.useRealTimers();
  });

  it("drops marks the roster has confirmed: gone, resumed, or running again", () => {
    const store = new CloseActionStore({
      archive: vi.fn(async () => undefined),
      destroy: vi.fn(async () => undefined),
    });
    store.act("archive", target("s.1"));
    store.act("archive", target("s.2"));
    store.act("archive", target("s.3", 3));
    const roster = [
      // s.1 is absent: gone from the roster — confirmed.
      {
        ...liveRow("s.2", 1),
        state: { type: "ended" as const, generation: 1, code: 0, integrity: null },
      },
      liveRow("s.3", 4),
    ];

    store.pruneConfirmed(roster as never);

    // s.1 dropped (absent), s.3 dropped (resumed). s.2's mark STAYS: an
    // ended row IS the archive's outcome, and the strip's own roster filter
    // keeps an archived row out — even one that was opened from History.
    expect(store.getClosingSnapshot()).toEqual(["s.2"]);
  });

  it("keeps a same-generation recovered row hidden across unrelated publications", () => {
    // A recovered transcript has no process, so the close fired without
    // asking; a mere republication of the same row must not bring it back.
    const store = new CloseActionStore({
      archive: vi.fn(async () => undefined),
      destroy: vi.fn(async () => undefined),
    });
    store.act("archive", target("s.1"));
    const recoveredRow = {
      id: "s.1",
      workspaceId: "workspace-1",
      kind: "acp" as const,
      title: "s.1",
      state: {
        type: "recovered" as const,
        generation: 1,
        integrity: {
          kind: "unverifiable" as const,
          droppedFrames: 0,
          droppedBytes: 0,
          trimmedBytes: 0,
        },
      },
      elapsedMs: null,
    };

    store.pruneConfirmed([recoveredRow]);

    expect(store.getClosingSnapshot()).toEqual(["s.1"]);

    // A resume — a new generation — is something really changing it.
    store.pruneConfirmed([{ ...recoveredRow, state: { ...recoveredRow.state, generation: 2 } }]);
    expect(store.getClosingSnapshot()).toEqual([]);
  });

  it("a late generation-1 rejection cannot unhide or fail generation 2", async () => {
    vi.useFakeTimers();
    let rejectG1!: (cause: unknown) => void;
    let rejectG2!: (cause: unknown) => void;
    const archive = vi
      .fn<() => Promise<void>>()
      .mockImplementationOnce(
        () =>
          new Promise((_, reject) => {
            rejectG1 = reject;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((_, reject) => {
            rejectG2 = reject;
          }),
      );
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });

    // Close generation 1; the roster resumes the session to generation 2
    // and prunes the stale mark; the user closes generation 2.
    store.act("archive", target("s.1", 1));
    store.pruneConfirmed([liveRow("s.1", 2)]);
    store.act("archive", target("s.1", 2));
    expect(store.getClosingSnapshot()).toEqual(["s.1"]);

    // Generation 1's promise rejects now: it must settle nothing.
    rejectG1(new Error("late refusal"));
    await settleStore(store);

    expect(store.getClosingSnapshot()).toEqual(["s.1"]);
    expect(store.getFailuresSnapshot()).toEqual([]);

    // And generation 2's own refusal still settles its own act.
    rejectG2(new Error("daemon refused stop"));
    await settleStore(store);
    expect(store.getClosingSnapshot()).toEqual([]);
    expect(store.getFailuresSnapshot()).toEqual([
      { id: "s.1", message: "Archive of “s.1” failed: daemon refused stop" },
    ]);
    vi.useRealTimers();
  });

  it("a success clears only its own generation's failure", async () => {
    vi.useFakeTimers();
    const archive = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error("refused one"))
      .mockResolvedValueOnce(undefined);
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });

    store.act("archive", target("s.1", 1));
    await settleStore(store);
    expect(store.getFailuresSnapshot()).toHaveLength(1);

    // A generation-2 close succeeds: the generation-1 line is not its
    // failure to clear. (Its staleness across a resume is recorded, not
    // fixed — see the report.)
    store.act("archive", target("s.1", 2));
    await settleStore(store);
    expect(store.getFailuresSnapshot()).toEqual([
      { id: "s.1", message: "Archive of “s.1” failed: refused one" },
    ]);
    vi.useRealTimers();
  });

  it("hands a refusal back to the caller", async () => {
    vi.useFakeTimers();
    const archive = vi.fn(async () => {
      throw new Error("daemon refused stop");
    });
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });
    const onFailed = vi.fn();

    store.act("archive", target("s.1"), onFailed);
    await settleStore(store);

    expect(onFailed).toHaveBeenCalledWith();
    vi.useRealTimers();
  });

  it("ignores a second act for a row that is already leaving", () => {
    const archive = vi.fn(async () => undefined);
    const store = new CloseActionStore({ archive, destroy: vi.fn(async () => undefined) });

    store.act("archive", target("s.1"));
    store.act("archive", target("s.1"));

    expect(archive).toHaveBeenCalledTimes(1);
  });

  it("names delete failures with the heavier verb", async () => {
    vi.useFakeTimers();
    const destroy = vi.fn(async () => {
      throw new Error("Close the session before deleting it.");
    });
    const store = new CloseActionStore({ archive: vi.fn(async () => undefined), destroy });

    store.act("delete", target("s.1"));
    await settleStore(store);

    expect(store.getFailuresSnapshot()).toEqual([
      { id: "s.1", message: "Delete of “s.1” failed: Close the session before deleting it." },
    ]);
    vi.useRealTimers();
  });
});

describe("discardPersistedPendingCloses", () => {
  it("removes an older build's undo records without reading or firing them", () => {
    const storage = { getItem: vi.fn(() => null), removeItem: vi.fn() };
    discardPersistedPendingCloses(() => storage);
    expect(storage.removeItem).toHaveBeenCalledWith(OLDER_BUILD_PENDING_KEY);
    expect(storage.getItem).not.toHaveBeenCalled();
  });

  it("swallows a restricted storage getter, and tolerates no storage at all", () => {
    expect(() =>
      discardPersistedPendingCloses(() => {
        throw new Error("storage is blocked");
      }),
    ).not.toThrow();
    expect(() => discardPersistedPendingCloses(() => null)).not.toThrow();
  });
});
