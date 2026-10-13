import { describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { dropAgentAnsweredCards, dropDuplicateFinishNotices, isAgentTraffic } from "./agentTraffic";

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

function system(text: string): Extract<AgentChatItem, { role: "system" }> {
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
  it("leaves a sub-agent's rows to the fold, and keeps a null parent id as the parent's row", () => {
    expect(
      isAgentTraffic({
        id: "a",
        role: "assistant",
        text: "child",
        messageId: null,
        parentToolUseId: "toolu-1",
      }),
    ).toBe(false);
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

  it("keeps a failed poll the person must see, and hides a failed answer or listing too", () => {
    const failed = (title: string, output: string): AgentChatItem => ({
      id: `failed-${title}`,
      role: "tool",
      title,
      output,
      toolCallId: title,
      status: "failed",
    });
    expect(isAgentTraffic(failed("mcp__devboule__devboule_agent_activity", ""))).toBe(false);
    expect(
      isAgentTraffic(failed("mcp__devboule__devboule_answer_permission", "Mcp error -32602")),
    ).toBe(true);
    expect(
      isAgentTraffic(
        failed("mcp__devboule__devboule_list_pending_permissions", "user rejected MCP tool call"),
      ),
    ).toBe(true);
  });

  it("hides the orchestrator's answer and listing calls for children's cards", () => {
    expect(isAgentTraffic(tool("mcp__devboule__devboule_answer_permission"))).toBe(true);
    expect(isAgentTraffic(tool("mcp__devboule__devboule_list_pending_permissions"))).toBe(true);
    expect(isAgentTraffic(tool("Read index.html"))).toBe(false);
  });

  it("hides creator wake notes and a running task row, and keeps other system rows", () => {
    expect(
      isAgentTraffic(
        system("The person approved the agent creation: 'Website' started as session s.1."),
      ),
    ).toBe(true);
    // A start that failed is the person's to see, so it is drawn as a failure.
    expect(isAgentTraffic(system("The agent creation did not start: no provider."))).toBe(false);
    expect(
      isAgentTraffic({
        ...system("Running agent Explore auth · test-model · 2 tools"),
        task: { kind: "agent" as const, state: "running" as const, title: "Explore auth" },
      }),
    ).toBe(true);
    expect(isAgentTraffic(system("Background agent finished · Explore auth · took 8s"))).toBe(
      false,
    );
    expect(isAgentTraffic(system("Codex declined an out-of-scope request."))).toBe(false);
  });

  it("hides a child's permission relay: the person answers the card in the child's tab", () => {
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
  });

  it("hides a child's needs-answer and quiet notices: the rail and the child's tab keep that state", () => {
    const childNotice = (kind: string) =>
      ({
        id: `n-${kind}`,
        role: "daemon_notice",
        notice: { recognized: true, kind, childSessionId: "s.child.1", childName: "worker" },
      }) as unknown as AgentChatItem;
    expect(isAgentTraffic(childNotice("agent_input_required"))).toBe(true);
    expect(isAgentTraffic(childNotice("agent_quiet"))).toBe(true);
  });

  it("hides an idle-close notice and keeps other notices", () => {
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
      {
        ...system("Background agent finished · worker · took 7m 0s"),
        task: {
          kind: "agent" as const,
          state: "finished" as const,
          title: "worker",
          durationMs: 420_000,
        },
      },
      finishNotice("worker"),
    ];
    expect(dropDuplicateFinishNotices(items)).toEqual([items[0], items[1]]);
  });

  it("keeps a finish notice whose turn draws no settled row, and the other turns' rows", () => {
    const items = [
      user("u1"),
      {
        ...system("Background agent finished · worker · took 7m 0s"),
        task: {
          kind: "agent" as const,
          state: "finished" as const,
          title: "worker",
          durationMs: 420_000,
        },
      },
      user("u2"),
      finishNotice("worker"),
    ];
    expect(dropDuplicateFinishNotices(items)).toEqual(items);
  });
});

describe("dropAgentAnsweredCards", () => {
  function card(id: string): AgentChatItem {
    return {
      id: `card-${id}`,
      role: "permission_request",
      cardId: id,
      toolTitle: "Grep",
      childName: "worker",
      excerpt: "",
      excerptState: "absent",
    };
  }

  function answer(output: string, status = "completed"): AgentChatItem {
    return {
      id: `answer-${output}`,
      role: "tool",
      title: "mcp__devboule__devboule_answer_permission",
      output,
      toolCallId: "answer",
      status,
    };
  }

  it("drops a card an agent answered, and keeps the card the answer did not name", () => {
    const items = [
      card("toolu_a"),
      card("toolu_b"),
      answer('{"state":"answered","cardId":"toolu_a"}'),
    ];
    expect(dropAgentAnsweredCards(items).map((item) => item.id)).toEqual([
      "card-toolu_b",
      'answer-{"state":"answered","cardId":"toolu_a"}',
    ]);
  });

  it("keeps a card whose answer failed, or whose answer reports no state", () => {
    const items = [
      card("toolu_a"),
      answer('{"state":"answered","cardId":"toolu_a"}', "failed"),
      card("toolu_c"),
      answer('{"cardId":"toolu_c"}'),
    ];
    expect(
      dropAgentAnsweredCards(items).filter((item) => item.role === "permission_request"),
    ).toHaveLength(2);
  });
});
