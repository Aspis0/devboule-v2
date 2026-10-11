import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { dropRecoveredFailures } from "./recoveredFailures";

function call(id: string, title: string, status: string): AgentChatItem {
  return { id, role: "tool", title, output: "", toolCallId: id, status };
}

function user(id: string): AgentChatItem {
  return { id, role: "user", text: "go", messageId: null };
}

describe("dropRecoveredFailures", () => {
  it("drops a failed call that the same call later completed in the turn", () => {
    const items = [
      user("u1"),
      call("t1", "mcp__devboule__devboule_create_agent", "failed"),
      call("t2", "mcp__devboule__devboule_create_agent", "completed"),
    ];
    expect(dropRecoveredFailures(items).map((item) => item.id)).toEqual(["u1", "t2"]);
  });

  it("keeps a failed call that nothing later completed", () => {
    const items = [
      user("u1"),
      call("t1", "npm test", "failed"),
      call("t2", "npm run lint", "completed"),
    ];
    expect(dropRecoveredFailures(items).map((item) => item.id)).toEqual(["u1", "t1", "t2"]);
  });

  it("keeps a failure whose later success falls in the next turn", () => {
    const items = [
      user("u1"),
      call("t1", "npm test", "failed"),
      user("u2"),
      call("t2", "npm test", "completed"),
    ];
    expect(dropRecoveredFailures(items).map((item) => item.id)).toEqual(["u1", "t1", "u2", "t2"]);
  });
});
