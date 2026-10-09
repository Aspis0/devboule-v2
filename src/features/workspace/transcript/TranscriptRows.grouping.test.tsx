// @vitest-environment happy-dom

// Consecutive calls keep their group's count, and the group opens on its own so
// every call shows its quiet line at once, without a click on the group first.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TranscriptRows } from "./TranscriptRows";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const names = { sessionById: new Map(), deviceNames: new Map() };

function tool(id: string, title: string): AgentChatItem {
  return {
    id,
    role: "tool",
    title,
    output: "",
    toolCallId: id,
    status: "completed",
    kind: "read",
    locations: [{ path: title }],
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

describe("TranscriptRows grouping", () => {
  it("opens a group of consecutive calls, showing each call's line", async () => {
    await act(async () =>
      root?.render(
        <TranscriptRows
          items={[tool("tool-1", "a.ts"), tool("tool-2", "b.ts"), tool("tool-3", "c.ts")]}
          recoveredAttach={false}
          pendingPlanToolCallId={null}
          a2aNames={names}
          fileLinks={null}
          transcriptEnded={false}
          streamingThoughtId={null}
        />,
      ),
    );

    const group = host.querySelector<HTMLDetailsElement>("details.workspace-chat-tool-group");
    if (group === null) throw new Error("three consecutive calls did not form a group");
    expect(group.open).toBe(true);
    const lines = group.querySelectorAll(
      ".workspace-chat-tool-group-body .workspace-chat-tool-label",
    );
    expect(lines).toHaveLength(3);
    expect(group.querySelector(".workspace-chat-tool-group-count")?.textContent).toBe(
      "3 tool calls",
    );
  });
});
