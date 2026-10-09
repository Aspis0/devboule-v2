// @vitest-environment happy-dom

// A line the person opened stays open when another call joins it into a group:
// the group draws that call with a new component, and the person's open must come along.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TranscriptRows } from "./TranscriptRows";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const names = { sessionById: new Map(), deviceNames: new Map() };

function tool(id: string, output: string): AgentChatItem {
  return {
    id,
    role: "tool",
    title: "search",
    output,
    toolCallId: id,
    status: "completed",
    kind: "search",
  };
}

function Transcript({ items }: { items: AgentChatItem[] }) {
  return (
    <div>
      <TranscriptRows
        items={items}
        recoveredAttach={false}
        pendingPlanToolCallId={null}
        a2aNames={names}
        fileLinks={null}
        transcriptEnded={false}
        streamingThoughtId={null}
      />
    </div>
  );
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
  await act(async () => root?.render(<Transcript items={items} />));
}

describe("TranscriptRows opened lines", () => {
  it("keeps a line the person opened open when a second call groups with it", async () => {
    const first = tool("tool-1", "first output");
    await show([first]);
    const lone = host.querySelector("details");
    if (lone === null) throw new Error("the lone call had nothing to open");
    await act(async () => {
      lone.open = true;
      lone.dispatchEvent(new Event("toggle"));
    });

    await show([first, tool("tool-2", "second output")]);

    const inner = host.querySelector<HTMLDetailsElement>(
      ".workspace-chat-tool-group-body .workspace-chat-tool-details",
    );
    if (inner === null) throw new Error("the first call did not come back in its group");
    expect(inner.open).toBe(true);
    expect(inner.querySelector(".workspace-chat-tool-output-line")?.textContent).toBe(
      "first output",
    );
  });
});
