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

describe("ThoughtRow", () => {
  it("starts collapsed and previews only the first line", async () => {
    const container = await renderThought("First line\nSecond line");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    expect(button?.getAttribute("aria-expanded")).toBe("false");
    expect(button?.textContent).toContain("Thought");
    expect(button?.textContent).toContain("First line");
    expect(button?.textContent).not.toContain("Second line");
    expect(container.querySelector(".workspace-chat-thought-body")?.hasAttribute("hidden")).toBe(
      true,
    );
  });

  it("expands and collapses by click with aria-expanded in sync", async () => {
    const container = await renderThought("First line\nSecond line");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("thought toggle did not render");
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector(".workspace-chat-thought-body")?.hasAttribute("hidden")).toBe(
      false,
    );
    await act(async () => button.click());
    expect(button.getAttribute("aria-expanded")).toBe("false");
  });

  it("handles an empty thought without a preview", async () => {
    const container = await renderThought("");
    const button = container.querySelector(".workspace-chat-thought-trigger");
    expect(button?.textContent).toBe("Thought");
  });

  it("labels a streaming thought Thinking and keeps it collapsed", async () => {
    const container = await renderThought("Still forming", true);
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
  });

  it("keeps an open row open as streamed chunks extend its text", async () => {
    const container = await renderThought("First chunk");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("thought toggle did not render");
    await act(async () => button.click());
    await act(async () =>
      root?.render(
        <ThoughtRow
          className="workspace-chat-entry workspace-chat-thought"
          isStreaming
          label="Thought"
          text="First chunk and second chunk"
        />,
      ),
    );
    expect(
      container.querySelector(".workspace-chat-thought-trigger")?.getAttribute("aria-expanded"),
    ).toBe("true");
    expect(container.querySelector(".workspace-chat-thought-body")?.textContent).toBe(
      "First chunk and second chunk",
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
      ".workspace-chat-thought-trigger",
      ".workspace-chat-thought-chevron",
      ".workspace-chat-thought-preview",
      ".workspace-chat-thought .workspace-chat-copy",
    ]);
    const container = await renderThought("A preview");
    const row = container.querySelector<HTMLElement>(".workspace-chat-thought");
    const button = container.querySelector<HTMLElement>(".workspace-chat-thought-trigger");
    const preview = container.querySelector<HTMLElement>(".workspace-chat-thought-preview");
    const body = container.querySelector<HTMLElement>(".workspace-chat-thought-body");
    if (row === null || button === null || preview === null || body === null) {
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
    expect(getComputedStyle(body).fontSize).toBe("11px");
    expect(getComputedStyle(body).fontFamily).not.toContain("JetBrains");
  });
});
