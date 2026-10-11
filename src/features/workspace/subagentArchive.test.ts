import { describe, expect, it, vi } from "vitest";
import { errorSentence } from "../../lib/errorSentence";
import type { Session, SessionState } from "../../types/ipc";
import { archiveTargets } from "./subagentArchive";

const finished = (generation: number): SessionState => ({
  type: "ended",
  generation,
  code: 0,
  integrity: { kind: "complete" },
});

const child = (id: string, state: SessionState, activity?: Session["activity"]): Session => ({
  id,
  workspaceId: "workspace-1",
  kind: "acp",
  title: id,
  createdBy: "parent",
  state,
  elapsedMs: null,
  activity,
});

const SHOWN_CHANGED = "It changed since you asked, so it was left open.";

describe("archiving the children the person was shown", () => {
  it("closes a child still at the generation the person saw", async () => {
    const close = vi.fn(async () => undefined);
    const sentences = await archiveTargets(
      [{ id: "a", generation: 1 }],
      [child("a", finished(1))],
      close,
    );
    expect(close).toHaveBeenCalledWith("a");
    expect(sentences.size).toBe(0);
  });

  it("leaves a child alone when the roster has moved to a newer generation", async () => {
    const close = vi.fn(async () => undefined);
    const sentences = await archiveTargets(
      [{ id: "a", generation: 1 }],
      [child("a", finished(2))],
      close,
    );
    expect(close).not.toHaveBeenCalled();
    expect(sentences.get("a")).toBe(SHOWN_CHANGED);
  });

  it("leaves a child alone when it is no longer finished", async () => {
    const close = vi.fn(async () => undefined);
    const running = child("a", { type: "live", generation: 1 }, "working");
    const sentences = await archiveTargets([{ id: "a", generation: 1 }], [running], close);
    expect(close).not.toHaveBeenCalled();
    expect(sentences.get("a")).toBe(SHOWN_CHANGED);
  });

  it("counts a child the roster no longer lists as closed, without a call", async () => {
    const close = vi.fn(async () => undefined);
    const sentences = await archiveTargets([{ id: "gone", generation: 1 }], [], close);
    expect(close).not.toHaveBeenCalled();
    expect(sentences.size).toBe(0);
  });

  it("answers the mapped sentence when the close fails, never the raw cause", async () => {
    const cause = { code: "journal", message: "raw journal words" };
    const sentences = await archiveTargets(
      [{ id: "a", generation: 1 }],
      [child("a", finished(1))],
      vi.fn(async () => {
        throw cause;
      }),
    );
    expect(sentences.get("a")).toBe(errorSentence(cause).sentence);
    expect(sentences.get("a")).not.toContain("raw journal words");
  });

  it("does nothing while the roster has not been read", async () => {
    const close = vi.fn(async () => undefined);
    const sentences = await archiveTargets([{ id: "a", generation: 1 }], undefined, close);
    expect(close).not.toHaveBeenCalled();
    expect(sentences.size).toBe(0);
  });
});
