import { describe, expect, it } from "vitest";
import type { Session } from "../../types/ipc";
import { isDescendantOf, isTopLevel, parentOf } from "./sessionAncestry";

function session(id: string, patch: Partial<Session> = {}): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "claude",
    title: id,
    state: { type: "live", generation: 1 },
    elapsedMs: null,
    contextId: id,
    ...patch,
  } as Session;
}

describe("who a session answers to", () => {
  it("names the agent that created it, and a human-started session has no parent", () => {
    expect(parentOf(session("child", { createdBy: "root" }))).toBe("root");
    expect(parentOf(session("root"))).toBeUndefined();
    expect(isTopLevel(session("root"))).toBe(true);
    expect(isTopLevel(session("child", { createdBy: "root" }))).toBe(false);
  });

  it("reads a legacy row without createdBy through its context, which is its creator's", () => {
    const legacy = session("legacy-child", { contextId: "root" });
    expect(parentOf(legacy)).toBe("root");
    expect(isTopLevel(legacy)).toBe(false);
  });

  it("finds a descendant at any depth, through creators and through legacy contexts", () => {
    const roster = [
      session("root"),
      session("child", { createdBy: "root" }),
      session("grandchild", { createdBy: "child" }),
      session("legacy-grandchild", { createdBy: undefined, contextId: "root" }),
      session("stranger"),
    ];
    expect(isDescendantOf(roster[2], "root", roster)).toBe(true);
    expect(isDescendantOf(roster[3], "root", roster)).toBe(true);
    expect(isDescendantOf(roster[4], "root", roster)).toBe(false);
    expect(isDescendantOf(roster[0], "root", roster)).toBe(false);
  });
});
