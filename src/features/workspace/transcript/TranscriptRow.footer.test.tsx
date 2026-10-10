// @vitest-environment happy-dom

// An answer carries no copy icon of its own: the completed turn's footer holds
// the one copy, and it ends the turn's last answer, always there, so nothing
// about the answer hides behind a hover.

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

async function renderAssistant(
  text: string,
  turnEnd?: { text: string; workedMs?: number },
): Promise<HTMLElement> {
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
        turnEnd={turnEnd}
      />,
    ),
  );
  return host;
}

describe("assistant row footer", () => {
  it("draws no copy icon of its own, so a paragraph is never the copy target", async () => {
    const container = await renderAssistant("The answer.");
    expect(container.querySelector(".workspace-chat-message-footer")).toBeNull();
    expect(container.querySelector(".timeline-copy-chip")).toBeNull();
  });

  it("ends a completed turn's last answer with the turn's footer, copy first", async () => {
    const container = await renderAssistant("The answer.", {
      text: "The answer.",
      workedMs: 10_000,
    });
    const message = container.querySelector<HTMLElement>(".workspace-chat-assistant");
    if (message === null) throw new Error("the message did not render");
    const footer = message.querySelector<HTMLElement>(".workspace-chat-turn-footer");
    if (footer === null) throw new Error("the turn had no footer");
    expect(message.lastElementChild).toBe(footer);
    expect(footer.firstElementChild?.classList.contains("timeline-copy-chip")).toBe(true);
    expect(footer.querySelector(".workspace-chat-turn-worked")?.textContent).toBe("Worked for 10s");
  });

  it("draws no footer for an empty message, nothing to copy", async () => {
    const container = await renderAssistant("");
    expect(container.querySelector(".workspace-chat-turn-footer")).toBeNull();
  });
});
