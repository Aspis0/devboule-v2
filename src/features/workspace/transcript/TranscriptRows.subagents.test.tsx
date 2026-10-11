// @vitest-environment happy-dom

// A sub-agent run with no tab of its own is one quiet line; its own words open on click.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TranscriptRows } from "./TranscriptRows";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const names = { sessionById: new Map(), deviceNames: new Map() };

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

function spawn(id: string, title: string): AgentChatItem {
  return {
    id: `spawn-${id}`,
    role: "tool",
    title,
    output: "",
    toolCallId: id,
    status: "completed",
    kind: "other",
  };
}

function childText(id: string, text: string, parent: string): AgentChatItem {
  return { id, role: "assistant", text, messageId: null, parentToolUseId: parent };
}

describe("a sub-agent run", () => {
  it("is one quiet line naming the run, with its own words behind the click", async () => {
    await show([
      { id: "u1", role: "user", text: "Explore it", messageId: null },
      spawn("toolu_task", "Explore auth"),
      childText("c1", "Found the token store.", "toolu_task"),
      childText("c2", "It refreshes on expiry.", "toolu_task"),
      { id: "a1", role: "assistant", text: "Here is the summary.", messageId: null },
    ]);

    const runLine = host.querySelector(".workspace-chat-quiet-line");
    expect(runLine?.textContent).toContain("Subagent · Explore auth");
    // The spawning call is the same operation as the run, so it does not draw a second line.
    expect(host.querySelectorAll(".workspace-chat-quiet-line")).toHaveLength(1);
    const details = runLine?.closest("details");
    expect(details?.open).toBe(false);
    expect(host.textContent).toContain("Here is the summary.");

    await act(async () => runLine?.querySelector("summary")?.click());
    expect(details?.open).toBe(true);
    expect(details?.textContent).toContain("Found the token store.");
    expect(details?.textContent).toContain("It refreshes on expiry.");
  });
});
