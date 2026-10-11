import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { dropRecoveredFailures } from "./recoveredFailures";

function call(id: string, title: string, status: string, command?: string): AgentChatItem {
  return {
    id,
    role: "tool",
    title,
    output: "",
    toolCallId: id,
    status,
    ...(command === undefined ? {} : { command }),
  };
}

function user(id: string): AgentChatItem {
  return { id, role: "user", text: "go", messageId: null };
}

function ids(items: AgentChatItem[]): string[] {
  return items.map((item) => item.id);
}

describe("dropRecoveredFailures", () => {
  it("drops a failed command that the same command later completed in the turn", () => {
    const items = [
      user("u1"),
      call("t1", "Shell", "failed", "npm test"),
      call("t2", "Shell", "completed", "npm test"),
    ];
    expect(ids(dropRecoveredFailures(items))).toEqual(["u1", "t2"]);
  });

  it("keeps a failed command when a different command with the same title succeeded", () => {
    const items = [
      user("u1"),
      call("t1", "Shell", "failed", "npm test"),
      call("t2", "Shell", "completed", "npm run lint"),
    ];
    expect(ids(dropRecoveredFailures(items))).toEqual(["u1", "t1", "t2"]);
  });

  it("keeps a failed Devboule call whose later success has the same title but no target", () => {
    // Two create calls for two children: the same title, and nothing that says which is which.
    const items = [
      user("u1"),
      call("t1", "mcp__devboule__devboule_create_agent", "failed"),
      call("t2", "mcp__devboule__devboule_create_agent", "completed"),
    ];
    expect(ids(dropRecoveredFailures(items))).toEqual(["u1", "t1", "t2"]);
  });

  it("does not count a cancelled call as the success that recovers", () => {
    const items = [
      user("u1"),
      call("t1", "Shell", "failed", "npm test"),
      call("t2", "Shell", "cancelled", "npm test"),
    ];
    expect(ids(dropRecoveredFailures(items))).toEqual(["u1", "t1", "t2"]);
  });

  it("keeps a failure whose later success falls in the next turn", () => {
    const items = [
      user("u1"),
      call("t1", "Shell", "failed", "npm test"),
      user("u2"),
      call("t2", "Shell", "completed", "npm test"),
    ];
    expect(ids(dropRecoveredFailures(items))).toEqual(["u1", "t1", "u2", "t2"]);
  });
});
