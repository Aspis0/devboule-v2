// @vitest-environment happy-dom

// An assistant message ends with one footer row that holds only its copy action,
// left-aligned and always there, so nothing about the answer hides behind a hover.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TranscriptRow } from "./TranscriptRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
  host = null;
});

async function renderAssistant(text: string): Promise<HTMLElement> {
  const item = { id: "assistant-1", role: "assistant", text } as AgentChatItem;
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () =>
    root?.render(
      <TranscriptRow
        entry={item}
        fileLinks={null}
        transcriptEnded={false}
        isStreamingThought={false}
      />,
    ),
  );
  return host;
}

describe("assistant message footer", () => {
  it("is the last row of the message and holds only the copy action, first", async () => {
    const container = await renderAssistant("The answer.");
    const message = container.querySelector<HTMLElement>(".workspace-chat-assistant");
    if (message === null) throw new Error("the message did not render");
    const footer = message.querySelector<HTMLElement>(".workspace-chat-message-footer");
    if (footer === null) throw new Error("the message had no footer");
    expect(message.lastElementChild).toBe(footer);
    expect(footer.children).toHaveLength(1);
    const copy = footer.firstElementChild;
    expect(copy?.classList.contains("timeline-copy-chip")).toBe(true);
    expect(copy?.getAttribute("aria-label")).toBe("Copy message");
  });

  it("draws no footer for an empty message, nothing to copy", async () => {
    const container = await renderAssistant("");
    expect(container.querySelector(".workspace-chat-message-footer")).toBeNull();
  });
});
