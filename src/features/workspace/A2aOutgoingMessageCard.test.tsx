// @vitest-environment happy-dom

// A message this agent sent to another agent is one closed line; its text waits
// behind the click, and no label speaks about "this session" or "words".

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { A2aOutgoingMessageCard } from "./A2aOutgoingMessageCard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root | null = null;

async function renderCard(text: string): Promise<HTMLElement> {
  root = createRoot(container);
  await act(async () => {
    root?.render(
      <A2aOutgoingMessageCard item={{ id: "out-1", role: "a2a_outgoing_message", text }} />,
    );
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

describe("A2aOutgoingMessageCard", () => {
  it("draws one closed line that says a message was sent", async () => {
    const shown = await renderCard("please check the hero image");
    const details = shown.querySelector("details");
    expect(details?.open).toBe(false);
    expect(shown.querySelector(".workspace-chat-activity-line")?.textContent).toBe(
      "Sent a message to another agent",
    );
  });

  it("speaks in no label about this session or its words, and never says unverified", async () => {
    const shown = await renderCard("please check the hero image");
    expect(shown.textContent).not.toContain("This session");
    expect(shown.textContent).not.toContain("this session");
    expect(shown.textContent).not.toContain("words");
    expect(shown.textContent).not.toContain("unverified");
    expect(shown.querySelector("figure")).toBeNull();
  });

  it("opens the sent text on the line's click", async () => {
    const shown = await renderCard("please check the hero image");
    expect(shown.textContent).not.toContain("please check the hero image");
    const details = shown.querySelector("details");
    if (details === null) throw new Error("the line had nothing to open");
    await act(async () => {
      details.open = true;
      details.dispatchEvent(new Event("toggle"));
    });
    expect(shown.querySelector(".workspace-chat-activity-body")?.textContent).toBe(
      "please check the hero image",
    );
  });
});
