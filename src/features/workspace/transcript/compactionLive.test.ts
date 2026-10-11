// A compaction start is still running only in the live turn. A start before a
// later user row belongs to a turn that moved on, so no second spinner can run
// next to a later "done" divider.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { groupToolCalls } from "../../../lib/toolCallGroups";
import { liveCompactionIds } from "./compactionState";

const start = (id: string): AgentChatItem => ({
  id,
  role: "system",
  text: "Compacting the context.",
  severity: "info",
});
const user = (id: string): AgentChatItem => ({ id, role: "user", text: "next", messageId: null });

describe("liveCompactionIds", () => {
  it("marks the start of the running last turn", () => {
    const entries = groupToolCalls([user("u1"), start("s1")]);
    expect([...liveCompactionIds(entries, true)]).toEqual(["s1"]);
  });

  it("marks nothing once the turn is no longer running", () => {
    const entries = groupToolCalls([user("u1"), start("s1")]);
    expect(liveCompactionIds(entries, false).size).toBe(0);
  });

  it("leaves a start stranded behind a later user row unmarked", () => {
    const entries = groupToolCalls([start("s1"), user("u2"), start("s2")]);
    expect([...liveCompactionIds(entries, true)]).toEqual(["s2"]);
  });
});
