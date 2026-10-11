import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { dropDuplicateFinishNotices, isAgentTraffic } from "./agentTraffic";

function tool(title: string, output = ""): AgentChatItem {
  return {
    id: `tool-${title}`,
    role: "tool",
    title,
    output,
    toolCallId: title,
    status: "completed",
  };
}

function system(text: string): AgentChatItem {
  return { id: `sys-${text}`, role: "system", text, severity: "info" };
}

function finishNotice(childName: string): AgentChatItem {
  return {
    id: `notice-${childName}`,
    role: "daemon_notice",
    notice: {
      recognized: true,
      kind: "agent_finished",
      childSessionId: "s.child.1",
      childName,
      state: "completed",
      summary: null,
      unattributed: null,
      truncated: false,
    },
  } as unknown as AgentChatItem;
}

function user(id: string): AgentChatItem {
  return { id, role: "user", text: "next", messageId: null };
}

describe("isAgentTraffic", () => {
  it("hides a sub-agent's own rows and keeps a null parent id as the parent's row", () => {
    expect(
      isAgentTraffic({
        id: "a",
        role: "assistant",
        text: "child",
        messageId: null,
        parentToolUseId: "toolu-1",
      }),
    ).toBe(true);
    expect(
      isAgentTraffic({
        id: "b",
        role: "assistant",
        text: "parent",
        messageId: null,
        parentToolUseId: "",
      }),
    ).toBe(false);
  });

  it("hides the poll of a running or idle agent, and shows one that is blocked", () => {
    const idle = tool("mcp__devboule__devboule_agent_activity", '{"activity":"idle"}');
    const blocked = tool("mcp__devboule__devboule_agent_activity", '{"activity":"blocked"}');
    expect(isAgentTraffic(idle)).toBe(true);
    expect(isAgentTraffic(blocked)).toBe(false);
  });

  it("hides the orchestrator's answer and listing calls for children's cards", () => {
    expect(isAgentTraffic(tool("mcp__devboule__devboule_answer_permissions"))).toBe(true);
    expect(isAgentTraffic(tool("mcp__devboule__devboule_list_pending_permissions"))).toBe(true);
    expect(isAgentTraffic(tool("Read index.html"))).toBe(false);
  });

  it("hides creator wake notes and a running task row, and keeps other system rows", () => {
    expect(
      isAgentTraffic(
        system("The person approved the agent creation: 'Website' started as session s.1."),
      ),
    ).toBe(true);
    expect(isAgentTraffic(system("The agent creation did not start: no provider."))).toBe(true);
    expect(isAgentTraffic(system("Running agent Explore auth · test-model · 2 tools"))).toBe(true);
    expect(isAgentTraffic(system("Background agent finished · Explore auth · took 8s"))).toBe(
      false,
    );
    expect(isAgentTraffic(system("Codex declined an out-of-scope request."))).toBe(false);
  });

  it("hides a permission relay and an idle-close notice, and keeps other notices", () => {
    expect(
      isAgentTraffic({
        id: "p",
        role: "permission_request",
        cardId: "toolu_1",
        toolTitle: "Grep",
        childName: "worker",
        excerpt: "",
        excerptState: "absent",
      }),
    ).toBe(true);
    const idleClosed = {
      id: "i",
      role: "daemon_notice",
      notice: {
        recognized: true,
        kind: "agent_idle_closed",
        childSessionId: "s.child.1",
        childName: "worker",
        idleMinutes: 30,
      },
    } as unknown as AgentChatItem;
    expect(isAgentTraffic(idleClosed)).toBe(true);
    expect(isAgentTraffic(finishNotice("worker"))).toBe(false);
  });
});

describe("dropDuplicateFinishNotices", () => {
  it("drops a finish notice when its turn already draws the child's settled row", () => {
    const items = [
      user("u1"),
      system("Background agent finished · worker · took 7m 0s"),
      finishNotice("worker"),
    ];
    expect(dropDuplicateFinishNotices(items)).toEqual([items[0], items[1]]);
  });

  it("keeps a finish notice whose turn draws no settled row, and the other turns' rows", () => {
    const items = [
      user("u1"),
      system("Background agent finished · worker · took 7m 0s"),
      user("u2"),
      finishNotice("worker"),
    ];
    expect(dropDuplicateFinishNotices(items)).toEqual(items);
  });
});
