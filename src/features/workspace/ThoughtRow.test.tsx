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

async function renderThought(text: string, isStreaming = false): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () =>
    root?.render(
      <ThoughtRow
        className="workspace-chat-entry workspace-chat-thought"
        text={text}
        isStreaming={isStreaming}
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

  it.each(["Enter", " "])("expands by %s and collapses by keyboard", async (key) => {
    const container = await renderThought("Reasoning");
    const button = container.querySelector<HTMLButtonElement>(".workspace-chat-thought-trigger");
    if (button === null) throw new Error("thought toggle did not render");
    await act(async () =>
      button.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key })),
    );
    expect(button.getAttribute("aria-expanded")).toBe("true");
    await act(async () =>
      button.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key })),
    );
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

  it("uses the thought-only mockup styles from the built bundle's stylesheet order", async () => {
    const { inject, token } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/app/errorBoundaries.css"),
      read("src/components/PermissionCard.css"),
      read("src/components/PickerChip.css"),
      read("src/features/design/artifactPreview.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/changes.css"),
      read("src/features/workspace/panel/files.css"),
      read("src/features/workspace/QueueTrack.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/history/history.css"),
      read("src/features/workspace/sidebar/sidebar.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    inject([
      ".workspace-chat-thought",
      ".workspace-chat-thought-trigger",
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
    expect(getComputedStyle(preview).textOverflow).toBe("ellipsis");
    expect(getComputedStyle(body).fontSize).toBe("11px");
    expect(getComputedStyle(body).fontFamily).not.toContain("JetBrains");
  });

  it("shows no invented duration", async () => {
    const container = await renderThought("A preview");
    expect(container.textContent).not.toMatch(/\b\d+\s*s\b/);
  });
});
