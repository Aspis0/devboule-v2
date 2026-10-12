// @vitest-environment happy-dom

// One footer per completed turn: a copy of the whole turn and, when this view
// saw it start, how long it ran. An answer's paragraphs carry no copy icon of
// their own.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { TurnCopyFooter } from "./TurnCopyFooter";
import { TranscriptRows } from "./TranscriptRows";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root | null = null;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
});

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  container.remove();
});

const NO_NAMES = { sessionById: new Map(), deviceNames: new Map() };

async function renderRows(items: AgentChatItem[], turnRunning = false): Promise<HTMLElement> {
  root = createRoot(container);
  await act(async () => {
    root?.render(
      <TranscriptRows
        items={items}
        recoveredAttach={false}
        a2aNames={NO_NAMES}
        fileLinks={null}
        transcriptEnded={false}
        streamingThoughtId={null}
        turnRunning={turnRunning}
      />,
    );
  });
  return container;
}

const user = (id: string, text: string): AgentChatItem => ({
  id,
  role: "user",
  text,
  messageId: null,
});

const answer = (id: string, text: string, workedMs?: number): AgentChatItem => ({
  id,
  role: "assistant",
  text,
  messageId: null,
  ...(workedMs === undefined ? {} : { workedMs }),
});

describe("TurnCopyFooter", () => {
  it("shows the copy and how long the turn worked", async () => {
    root = createRoot(container);
    await act(async () => {
      root?.render(<TurnCopyFooter text="the answer" workedMs={10_000} />);
    });
    expect(
      container.querySelector(".workspace-chat-turn-footer .timeline-copy-chip"),
    ).not.toBeNull();
    expect(container.querySelector(".workspace-chat-turn-worked")?.textContent).toBe(
      "Worked for 10s",
    );
  });

  it("shows only the copy when the turn's length is not known", async () => {
    root = createRoot(container);
    await act(async () => {
      root?.render(<TurnCopyFooter text="the answer" />);
    });
    expect(
      container.querySelector(".workspace-chat-turn-footer .timeline-copy-chip"),
    ).not.toBeNull();
    expect(container.querySelector(".workspace-chat-turn-worked")).toBeNull();
  });

  it("says the turn was stopped, with its length, when the provider stopped it abnormally", async () => {
    root = createRoot(container);
    await act(async () => {
      root?.render(<TurnCopyFooter text="partial" workedMs={274_000} stopReason="max_tokens" />);
    });
    const stopped = container.querySelector(".workspace-chat-turn-stopped");
    expect(stopped?.textContent).toBe("Stopped after 4m 34s");
    expect(stopped?.getAttribute("title")).toBe("Stopped: max_tokens");
    expect(container.querySelector(".workspace-chat-turn-worked")).toBeNull();
  });

  it("says only that it stopped when its length is not known", async () => {
    root = createRoot(container);
    await act(async () => {
      root?.render(<TurnCopyFooter text="partial" stopReason="max_tokens" />);
    });
    expect(container.querySelector(".workspace-chat-turn-stopped")?.textContent).toBe("Stopped");
  });

  it("keeps Worked for after a normal end, whatever the provider's word for it", async () => {
    root = createRoot(container);
    await act(async () => {
      root?.render(<TurnCopyFooter text="the answer" workedMs={10_000} stopReason="end_turn" />);
    });
    expect(container.querySelector(".workspace-chat-turn-worked")?.textContent).toBe(
      "Worked for 10s",
    );
    expect(container.querySelector(".workspace-chat-turn-stopped")).toBeNull();
  });
});

describe("the transcript's turn footers", () => {
  it("draws one footer for a completed turn, after its last answer, and none per paragraph", async () => {
    const shown = await renderRows([
      user("u1", "question"),
      answer("a1", "first paragraph"),
      answer("a2", "second paragraph", 3_000),
    ]);
    expect(shown.querySelectorAll(".workspace-chat-turn-footer")).toHaveLength(1);
    expect(shown.querySelectorAll(".workspace-chat-message-footer")).toHaveLength(0);
    expect(shown.querySelector(".workspace-chat-turn-worked")?.textContent).toBe("Worked for 3s");
    // The footer closes the turn: it sits in the last answer, not the first.
    const answers = shown.querySelectorAll(".workspace-chat-assistant");
    expect(answers).toHaveLength(2);
    expect(answers[0]?.querySelector(".workspace-chat-turn-footer")).toBeNull();
    expect(answers[1]?.querySelector(".workspace-chat-turn-footer")).not.toBeNull();
  });

  it("draws no footer for a turn still running", async () => {
    const shown = await renderRows([user("u1", "question"), answer("a1", "still writing")], true);
    expect(shown.querySelector(".workspace-chat-turn-footer")).toBeNull();
  });

  it("keeps the earlier turn's footer while a later turn runs", async () => {
    const shown = await renderRows(
      [
        user("u1", "first"),
        answer("a1", "first answer", 4_000),
        user("u2", "second"),
        answer("a2", "still writing"),
      ],
      true,
    );
    expect(shown.querySelectorAll(".workspace-chat-turn-footer")).toHaveLength(1);
  });
});
