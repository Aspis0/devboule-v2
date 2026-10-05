// One quiet line per compaction event: the daemon's start notice drops out
// wherever its finish notice directly follows, and nothing else moves.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { collapseCompactNotices } from "./useTranscriptEntries";

function system(id: string, text: string): AgentChatItem {
  return { id, role: "system", text, severity: "info" };
}

function user(id: string, text: string): AgentChatItem {
  return { id, role: "user", text, messageId: null };
}

const START = "Compacting the context.";
const FINISH = "Context compacted.";
const MANUAL_START = "Context manually compacted";
const MANUAL_FINISH = "Compacted";

describe("collapseCompactNotices", () => {
  it("drops the start notice an adjacent finish notice follows", () => {
    const items = [
      user("u1", "/compact"),
      system("s1", START),
      system("s2", FINISH),
      user("u2", "hi"),
    ];

    expect(collapseCompactNotices(items).map((item) => item.id)).toEqual(["u1", "s2", "u2"]);
  });

  it("collapses the manual compaction pair the same way", () => {
    const items = [user("u1", "/compact"), system("s1", MANUAL_START), system("s2", MANUAL_FINISH)];

    expect(collapseCompactNotices(items).map((item) => item.id)).toEqual(["u1", "s2"]);
  });

  it("keeps a start notice with no finish behind it", () => {
    const items = [user("u1", "/compact"), system("s1", START)];

    expect(collapseCompactNotices(items).map((item) => item.id)).toEqual(["u1", "s1"]);
  });

  it("keeps both lines when another row sits between them", () => {
    const items = [system("s1", START), user("u1", "wait"), system("s2", FINISH)];

    expect(collapseCompactNotices(items).map((item) => item.id)).toEqual(["s1", "u1", "s2"]);
  });

  it("leaves every other system line alone", () => {
    const items = [
      system("s1", "Goal set: one word"),
      system("s2", "Goal cleared."),
      system("s3", FINISH),
    ];

    expect(collapseCompactNotices(items).map((item) => item.id)).toEqual(["s1", "s2", "s3"]);
  });
});
