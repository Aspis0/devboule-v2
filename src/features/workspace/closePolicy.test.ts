// The close policy: a terminal close asks; an agent with a process — `live`
// or `silent` — asks, because silence alone is not idleness (the daemon
// flips Running to Silent on an output threshold, and no roster field says a
// turn has ended); an agent without a process closes without asking; a
// delete always asks.

import { describe, expect, it } from "vitest";
import type { Session } from "../../types/ipc";
import { closeNeedsConfirmation } from "./closePolicy";

function session(kind: Session["kind"], state: Session["state"]): Session {
  return {
    id: "s1",
    workspaceId: "workspace-1",
    kind,
    title: "one",
    state,
    elapsedMs: 0,
  };
}

describe("closeNeedsConfirmation", () => {
  it("asks before closing a terminal, whatever its state", () => {
    expect(
      closeNeedsConfirmation(session("terminal", { type: "live", generation: 1 }), "archive"),
    ).toBe(true);
  });

  it("asks before archiving a running agent", () => {
    expect(closeNeedsConfirmation(session("acp", { type: "live", generation: 1 }), "archive")).toBe(
      true,
    );
  });

  it("asks before archiving a silent agent: silence is not idleness", () => {
    // The daemon marks a stream silent on an output threshold alone — a long
    // tool call or a waiting provider looks exactly like this and is still
    // working.
    expect(
      closeNeedsConfirmation(session("acp", { type: "silent", generation: 1 }), "archive"),
    ).toBe(true);
  });

  it("archives an agent without a process at once", () => {
    expect(
      closeNeedsConfirmation(
        session("acp", {
          type: "recovered",
          generation: 1,
          integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
        }),
        "archive",
      ),
    ).toBe(false);
    expect(
      closeNeedsConfirmation(
        session("acp", { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } }),
        "archive",
      ),
    ).toBe(false);
  });

  it("always asks before a delete, which destroys the session", () => {
    const live = session("acp", { type: "live", generation: 1 });
    const silent = session("terminal", { type: "silent", generation: 1 });
    expect(closeNeedsConfirmation(live, "delete")).toBe(true);
    expect(closeNeedsConfirmation(silent, "delete")).toBe(true);
  });
});
