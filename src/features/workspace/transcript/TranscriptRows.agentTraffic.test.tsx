// @vitest-environment happy-dom

// Agent-to-agent traffic stays in the journal; the person's chat draws none of it.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TranscriptRows } from "./TranscriptRows";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const names = { sessionById: new Map(), deviceNames: new Map() };

const CREATOR_WAKE =
  "The person approved the agent creation: 'Website redesign' started as session s.process-34996.00000002-dc924750d02e785e. Its finish reaches you as a message that wakes you.";

function assistant(id: string, text: string, parentToolUseId?: string): AgentChatItem {
  return {
    id,
    role: "assistant",
    text,
    messageId: null,
    ...(parentToolUseId === undefined ? {} : { parentToolUseId }),
  };
}

function system(id: string, text: string): Extract<AgentChatItem, { role: "system" }> {
  return { id, role: "system", text, severity: "info" };
}

function devbouleTool(id: string, title: string, output: string): AgentChatItem {
  return {
    id,
    role: "tool",
    title,
    output,
    toolCallId: id,
    status: "completed",
    kind: "other",
  };
}

let root: Root | null = null;
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  host.remove();
});

async function show(items: AgentChatItem[]): Promise<void> {
  await act(async () =>
    root?.render(
      <TranscriptRows
        items={items}
        recoveredAttach={false}
        a2aNames={names}
        fileLinks={null}
        transcriptEnded={false}
        streamingThoughtId={null}
      />,
    ),
  );
}

describe("agent-to-agent traffic is not drawn", () => {
  it("drops the creator wake note that a created agent's start leaves behind", async () => {
    // The daemon's wake reaches the creator as a system_notice; the reducer keeps it as a plain system row.
    await show([assistant("msg-1", "Creating the agent."), system("sys-1", CREATOR_WAKE)]);

    expect(host.textContent).toContain("Creating the agent.");
    expect(host.textContent).not.toContain("The person approved the agent creation");
    expect(host.textContent).not.toContain("s.process-34996");
  });

  it("drops the creator wake note of a creation that did not start", async () => {
    await show([system("sys-1", "The agent creation did not start: the provider is required.")]);

    expect(host.textContent).not.toContain("The agent creation did not start");
  });

  it("drops a running agent's task row; the Tasks panel keeps its state", async () => {
    await show([
      {
        ...system("sys-1", "Running agent Website redesign · test-model · 2 tools"),
        task: { kind: "agent" as const, state: "running" as const, title: "Website redesign" },
      },
    ]);

    expect(host.textContent).not.toContain("Running agent");
  });

  it("drops the sub-agent's own text, which is read in the child's tab", async () => {
    await show([assistant("msg-1", "The child's private words.", "toolu_child")]);

    expect(host.textContent).not.toContain("The child's private words.");
  });

  it("drops the answer and poll calls of the orchestrator, grouped or not", async () => {
    // Two calls in a row would form a group; the group's own markup must be absent too.
    await show([
      devbouleTool("tool-1", "mcp__devboule__devboule_answer_permissions", '{"answered":true}'),
      devbouleTool("tool-2", "mcp__devboule__devboule_list_pending_permissions", "[]"),
    ]);

    expect(
      host.querySelector(".workspace-chat-tool-group, .workspace-chat-tool-summary"),
    ).toBeNull();
    expect(host.textContent).not.toContain("mcp__devboule__");
  });

  it("keeps a child's permission request that still waits for an answer", async () => {
    await show([
      {
        id: "permission-1",
        role: "permission_request",
        cardId: "toolu_01Covr",
        toolTitle: "Grep",
        childName: "Kalsa design concepts",
        excerpt: "(?i)kalsa",
        excerptState: "closed",
      },
    ]);

    expect(host.querySelector("[data-testid='agent-permission-request']")).not.toBeNull();
  });

  it("drops a child's permission request that an agent answered, with the answer", async () => {
    await show([
      {
        id: "permission-1",
        role: "permission_request",
        cardId: "toolu_01Covr",
        toolTitle: "Grep",
        childName: "Kalsa design concepts",
        excerpt: "",
        excerptState: "absent",
      },
      devbouleTool(
        "tool-1",
        "mcp__devboule__devboule_answer_permissions",
        '{"state":"answered","cardId":"toolu_01Covr"}',
      ),
    ]);

    expect(host.querySelector("[data-testid='agent-permission-request']")).toBeNull();
    expect(host.textContent).not.toContain("Relayed");
  });

  it("keeps the person's and the agent's own messages", async () => {
    await show([
      { id: "user-1", role: "user", text: "Make two mockups", messageId: null },
      assistant("msg-1", "Here they are."),
      system("sys-2", "Codex declined an out-of-scope request."),
    ]);

    expect(host.textContent).toContain("Make two mockups");
    expect(host.textContent).toContain("Here they are.");
    expect(host.textContent).toContain("Codex declined an out-of-scope request.");
  });
});
