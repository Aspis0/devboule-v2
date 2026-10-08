// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "./cssProof";
import { ThoughtRow } from "./ThoughtRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host.remove();
  removeCssProof();
});

async function renderThought(
  text: string,
  isStreaming = false,
  label = "Thought",
): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () =>
    root?.render(
      <ThoughtRow
        className="workspace-chat-entry workspace-chat-thought"
        isStreaming={isStreaming}
        label={label}
        text={text}
      />,
    ),
  );
  return host;
}

function occurrences(container: HTMLElement, needle: string): number {
  return container.textContent.split(needle).length - 1;
}

describe("ThoughtRow", () => {
  it("starts collapsed, previews only the first line, and leaves the rest unmounted", async () => {
    const container = await renderThought("First line\nSecond line");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    expect(button?.getAttribute("aria-expanded")).toBe("false");
    expect(button?.textContent).toContain("Thought");
    expect(button?.textContent).toContain("First line");
    expect(button?.textContent).not.toContain("Second line");
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
    expect(occurrences(container, "First line")).toBe(1);
    expect(occurrences(container, "Second line")).toBe(0);
  });

  it("expands to the full text in the box and drops the preview from the row", async () => {
    const container = await renderThought("First line\nSecond line");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("thought toggle did not render");
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("true");
    expect(button.textContent).toBe("Thought");
    expect(container.querySelector(".workspace-chat-thought-body")?.textContent).toBe(
      "First line\nSecond line",
    );
    expect(occurrences(container, "First line")).toBe(1);
    expect(occurrences(container, "Second line")).toBe(1);
  });

  it("collapses back to the preview by click with aria-expanded in sync", async () => {
    const container = await renderThought("First line\nSecond line");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("thought toggle did not render");
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("true");
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("false");
    expect(button.textContent).toContain("First line");
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
  });

  it("keeps a single-line thought as one row with the text and no box", async () => {
    const container = await renderThought("The user is just saying hello casually.");
    expect(container.querySelector(".workspace-chat-thought-trigger")).toBeNull();
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
    expect(container.querySelector(".workspace-chat-thought-label")?.textContent).toBe("Thought");
    expect(container.querySelector(".workspace-chat-thought-text")?.textContent).toBe(
      "The user is just saying hello casually.",
    );
    expect(occurrences(container, "The user is just saying hello casually.")).toBe(1);
  });

  it("treats a trailing blank line as still single-line", async () => {
    const container = await renderThought("Only line\n\n   \n");
    expect(container.querySelector(".workspace-chat-thought-trigger")).toBeNull();
    expect(occurrences(container, "Only line")).toBe(1);
  });

  it("shows only the label for an empty thought", async () => {
    const container = await renderThought("");
    expect(container.querySelector(".workspace-chat-thought-trigger")).toBeNull();
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
    expect(container.textContent).toBe("Thought");
  });

  it("labels a streaming single-line thought Thinking without a box", async () => {
    const container = await renderThought("Still forming", true);
    expect(container.querySelector(".workspace-chat-thought-status")?.textContent).toBe(
      "Thinking…",
    );
    expect(container.querySelector(".workspace-chat-thought-trigger")).toBeNull();
    expect(occurrences(container, "Still forming")).toBe(1);
  });

  it("labels a streaming multi-line thought Thinking and keeps it collapsed", async () => {
    const container = await renderThought("Still forming\nmore", true);
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    expect(button?.textContent).toContain("Thinking…");
    expect(button?.getAttribute("aria-expanded")).toBe("false");
  });

  it("preserves the item's subagent label and unavailable-depth copy", async () => {
    const container = await renderThought(
      "Checking files",
      false,
      "Subagent thought · depth unavailable",
    );
    expect(container.querySelector(".workspace-chat-thought-label")?.textContent).toBe(
      "Subagent thought · depth unavailable",
    );
  });

  it("uses the first non-empty trimmed line as its preview", async () => {
    const container = await renderThought("\n   Useful thought  \nAnother thought");
    expect(container.querySelector(".workspace-chat-thought-preview")?.textContent).toBe(
      "Useful thought",
    );
  });

  it("omits a preview and separator when the thought is whitespace only", async () => {
    const container = await renderThought("   \n\t  ");
    expect(container.querySelector(".workspace-chat-thought-preview")).toBeNull();
    expect(container.querySelector(".workspace-chat-thought-trigger")).toBeNull();
  });

  it("gains a box once a second line arrives, collapsed", async () => {
    const container = await renderThought("One line");
    expect(container.querySelector(".workspace-chat-thought-trigger")).toBeNull();
    await act(async () =>
      root?.render(
        <ThoughtRow
          className="workspace-chat-entry workspace-chat-thought"
          isStreaming
          label="Thought"
          text={"One line\nAnd another"}
        />,
      ),
    );
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    expect(button?.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector(".workspace-chat-thought-body")).toBeNull();
    expect(occurrences(container, "One line")).toBe(1);
  });

  it("keeps an open row open as streamed chunks extend its text", async () => {
    const container = await renderThought("First chunk\nsecond chunk");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("thought toggle did not render");
    await act(async () => button.click());
    await act(async () =>
      root?.render(
        <ThoughtRow
          className="workspace-chat-entry workspace-chat-thought"
          isStreaming
          label="Thought"
          text={"First chunk\nsecond chunk and a third"}
        />,
      ),
    );
    expect(
      container.querySelector(".workspace-chat-thought-trigger")?.getAttribute("aria-expanded"),
    ).toBe("true");
    expect(container.querySelector(".workspace-chat-thought-body")?.textContent).toBe(
      "First chunk\nsecond chunk and a third",
    );
  });

  it("uses thought styles from Workspace.css and resolved theme tokens", async () => {
    const { inject, token } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/Workspace.css"),
    ]);
    inject([
      ".workspace-chat-thought",
      ".workspace-chat-thought .workspace-chat-thought-trigger",
      ".workspace-chat-thought-chevron",
      ".workspace-chat-thought-preview",
      ".workspace-chat-thought-line",
      ".workspace-chat-thought-text",
      ".workspace-chat-thought .workspace-chat-copy",
    ]);
    const container = await renderThought("A preview\nMore detail");
    const row = container.querySelector<HTMLElement>(".workspace-chat-thought");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    const preview = container.querySelector<HTMLElement>(".workspace-chat-thought-preview");
    if (row === null || button === null || preview === null) {
      throw new Error("thought row styles have no rendered target");
    }
    expect(getComputedStyle(row).fontSize).toBe("12px");
    expect(getComputedStyle(row).color).toBe(token("--muted"));
    expect(getComputedStyle(button).fontFamily).not.toContain("JetBrains");
    expect(getComputedStyle(button).paddingLeft).toBe("2px");
    expect(getComputedStyle(button).paddingRight).toBe("2px");
    const chevron = button.querySelector<SVGElement>(".workspace-chat-thought-chevron");
    expect(chevron).not.toBeNull();
    if (chevron !== null) expect(getComputedStyle(chevron).width).toBe("12px");
    expect(getComputedStyle(preview).textOverflow).toBe("ellipsis");
    await act(async () => button.click());
    const body = container.querySelector<HTMLElement>(".workspace-chat-thought-body");
    if (body === null) throw new Error("expanded thought body did not render");
    expect(getComputedStyle(body).fontSize).toBe("12px");
    expect(getComputedStyle(body).fontFamily).not.toContain("JetBrains");
  });

  it("wraps a single-line thought in the row instead of ellipsizing it", async () => {
    const { inject } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/Workspace.css"),
    ]);
    inject([".workspace-chat-thought-line", ".workspace-chat-thought-text"]);
    const container = await renderThought("A single line of reasoning that is long");
    const text = container.querySelector<HTMLElement>(".workspace-chat-thought-text");
    if (text === null) throw new Error("single-line thought text did not render");
    expect(getComputedStyle(text).whiteSpace).not.toBe("nowrap");
    expect(getComputedStyle(text).textOverflow).not.toBe("ellipsis");
  });
});
