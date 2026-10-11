// @vitest-environment happy-dom

// Devboule's own agent calls read as one line each, with the raw result behind
// the line's click: never the tool's name, never the JSON on the line.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root | null = null;

function tool(overrides: Partial<ToolChatItem>): ToolChatItem {
  return {
    id: "t1",
    role: "tool",
    title: "devboule_send_message",
    output: "",
    toolCallId: "t1",
    status: "completed",
    ...overrides,
  } as ToolChatItem;
}

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  root = createRoot(container);
  await act(async () => {
    root?.render(<ToolRow item={item} transcriptEnded={false} />);
  });
  return container;
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
});

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  container.remove();
});

describe("ToolRow for Devboule's agent tools", () => {
  it("draws the call as one readable line naming the agent, with no raw tool name", async () => {
    const shown = await renderRow(
      tool({
        title: "mcp__devboule__devboule_send_message",
        output: '{"displayName":"Finish Kalsa website","delivered":true}',
      }),
    );
    const line = shown.querySelector(".workspace-chat-tool-label");
    expect(line?.textContent).toBe("Sent a message to Finish Kalsa website");
    expect(shown.textContent).not.toContain("devboule_send_message");
    expect(shown.textContent).not.toContain("delivered");
  });

  it("keeps a check call's raw result off the line, since the line opens the child", async () => {
    const shown = await renderRow(
      tool({
        title: "devboule_agent_activity",
        output:
          '{"sessionId":"s.f4.child","displayName":"Finish Kalsa website","activity":"working"}',
      }),
    );
    expect(shown.querySelector(".workspace-chat-tool-body")).toBeNull();
    expect(shown.querySelector("details")).toBeNull();
  });

  it("leaves another tool's row as it was", async () => {
    const shown = await renderRow(
      tool({ title: "cargo test", kind: "execute", command: "cargo test", output: "" }),
    );
    expect(shown.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Run");
  });
});

describe("the agent line's shape", () => {
  it("draws a Devboule agent call with the same agent icon slot as the agent cards", async () => {
    const shown = await renderRow(
      tool({ title: "devboule_send_message", output: '{"displayName":"Finish Kalsa website"}' }),
    );
    expect(shown.querySelector(".workspace-chat-tool-icon")).toBeNull();
    expect(shown.querySelector(".workspace-chat-agent-icon")).not.toBeNull();
  });
});
