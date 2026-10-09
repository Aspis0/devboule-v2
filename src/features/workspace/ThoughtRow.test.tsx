// @vitest-environment happy-dom

// A thought is one quiet row, "Thinking", with no words of its own until it is
// opened. Opened, its text sits once, in a box, under a chevron header.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { ThoughtRow } from "./ThoughtRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
  host = null;
});

async function renderThought(
  text: string,
  options: { isStreaming?: boolean; label?: string } = {},
): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () =>
    root?.render(
      <ThoughtRow
        label={options.label ?? "Thinking"}
        className="workspace-chat-entry workspace-chat-thought"
        text={text}
        isStreaming={options.isStreaming ?? false}
      />,
    ),
  );
  return host;
}

function occurrences(haystack: string, needle: string): number {
  return haystack.split(needle).length - 1;
}

describe("ThoughtRow", () => {
  it("is a collapsed row that reads Thinking and carries none of the text", async () => {
    const container = await renderThought("The user is just saying hello casually.");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    expect(button?.getAttribute("aria-expanded")).toBe("false");
    expect(button?.textContent).toBe("Thinking");
    expect(container.textContent).not.toContain("just saying hello");
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
  });

  it("collapses a multi-line thought the same way, with no preview line", async () => {
    const container = await renderThought("First line\nSecond line");
    expect(container.querySelector(".workspace-chat-thought-trigger")?.textContent).toBe(
      "Thinking",
    );
    expect(container.textContent).not.toContain("First line");
    expect(container.querySelector(".workspace-chat-thought-preview")).toBeNull();
  });

  it("shows the text once when opened, inside the box, and the header turns to a chevron", async () => {
    const container = await renderThought("First line\nSecond line");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("the thought had no trigger");
    await act(async () => button.click());

    expect(button.getAttribute("aria-expanded")).toBe("true");
    const body = container.querySelector<HTMLElement>(".workspace-chat-thought-body");
    expect(body?.textContent).toBe("First line\nSecond line");
    expect(occurrences(container.textContent ?? "", "First line")).toBe(1);
    expect(button.querySelector(".workspace-chat-thought-chevron")).not.toBeNull();
    expect(
      container.querySelector(".workspace-chat-thought")?.classList.contains("is-expanded"),
    ).toBe(true);
  });

  it("folds back to the bare row when the person closes it", async () => {
    const container = await renderThought("First line\nSecond line");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("the thought had no trigger");
    await act(async () => button.click());
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
    expect(button.querySelector(".workspace-chat-thought-chevron")).toBeNull();
  });

  it("shows a single-line thought under the same trigger, not as a bare text row", async () => {
    const container = await renderThought("The user is just saying hello casually.");
    expect(container.querySelector(".workspace-chat-thought-trigger")).not.toBeNull();
    expect(container.querySelector(".workspace-chat-thought-text")).toBeNull();
  });

  it("marks a streaming thought with an ellipsis after its label and keeps it collapsed", async () => {
    const container = await renderThought("Still forming\nmore", { isStreaming: true });
    expect(container.querySelector(".workspace-chat-thought-trigger")?.textContent).toBe(
      "Thinking…",
    );
    expect(container.querySelector(".workspace-chat-thought-status")?.textContent).toBe("…");
    expect(container.textContent).not.toContain("Still forming");
  });

  it("is a plain Thinking label, with no trigger and no box, when there is no text", async () => {
    const container = await renderThought("   \n\t  ");
    expect(container.querySelector(".workspace-chat-thought-trigger")).toBeNull();
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
    expect(container.textContent).toBe("Thinking");
  });

  it("takes a subagent label as given", async () => {
    const container = await renderThought("A thought", {
      label: "Subagent thinking · depth unavailable",
    });
    expect(container.querySelector(".workspace-chat-thought-trigger")?.textContent).toBe(
      "Subagent thinking · depth unavailable",
    );
  });

  it("carries a decorative icon on the row, not a text glyph", async () => {
    const container = await renderThought("A thought");
    const icon = container.querySelector(".workspace-chat-thought-trigger svg");
    expect(icon?.getAttribute("aria-hidden")).toBe("true");
  });
});
