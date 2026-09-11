// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ARTIFACT_CSP } from "./artifactCsp";
import { buildStandaloneArtifactHtml } from "./artifactExport";
import { ArtifactCopyControl } from "./ArtifactCopyControl";

const FRAGMENT = "<main><h1>Our menu</h1></main>";
const RUN_TITLE = "Agent did not report written files";

describe("ArtifactCopyControl", () => {
  let container: HTMLDivElement;
  let root: Root;
  let clipboardWrites: string[];
  let clipboardImpl: (text: string) => Promise<void>;
  const realClipboard = navigator.clipboard;

  beforeEach(() => {
    clipboardWrites = [];
    clipboardImpl = async (text: string) => {
      clipboardWrites.push(text);
    };
    // Same stub shape as DesignSurface.test.tsx and DiagnosticsPanel.test.tsx:
    // happy-dom ships a clipboard that tests must not reach into, so the
    // property is replaced and restored, not spied on.
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: (text: string) => clipboardImpl(text) },
    });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: realClipboard });
    vi.useRealTimers();
  });

  async function render(): Promise<void> {
    await act(async () => {
      root.render(<ArtifactCopyControl html={FRAGMENT} title={RUN_TITLE} />);
    });
  }

  function copyButton(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>('button[aria-label="Copy HTML"]');
    if (button === null) throw new Error("the copy button is missing");
    return button;
  }

  async function click(): Promise<void> {
    await act(async () => {
      copyButton().click();
    });
    await act(async () => undefined);
  }

  it("offers one labelled button and says nothing before it is used", async () => {
    await render();

    expect(container.querySelectorAll("button")).toHaveLength(1);
    expect(copyButton().textContent).toBe("Copy HTML");
    expect(copyButton().title).toBe("Copy the generated page as a standalone HTML document");
    expect(container.textContent).toBe("Copy HTML");
    expect(clipboardWrites).toHaveLength(0);
  });

  it("puts the exported document on the clipboard, not the canvas fragment", async () => {
    await render();

    await click();

    // The content, not just the call: a control that wrote the fragment, the
    // raw HTML string, or an empty document would still "have written
    // something". Equality with the export's own builder pins which document
    // landed, and the substrings say what that document is.
    expect(clipboardWrites).toHaveLength(1);
    const copied = clipboardWrites[0] ?? "";
    expect(copied).toBe(buildStandaloneArtifactHtml(FRAGMENT, RUN_TITLE));
    expect(copied).toContain("<!DOCTYPE html>");
    expect(copied).toContain('<html lang="en">');
    expect(copied).toContain('<meta charset="utf-8">');
    expect(copied).toContain('<meta name="viewport"');
    expect(copied).toContain("<title>Our menu</title>");
    expect(copied).toContain("<main><h1>Our menu</h1></main>");
    expect(copied).toContain(ARTIFACT_CSP);
  });

  it("tells the user the copy happened", async () => {
    await render();

    await click();

    expect(container.textContent).toBe("Copy HTMLCopied.");
    const status = container.querySelector('[role="status"]');
    expect(status?.textContent).toBe("Copied.");
  });

  it("reports a denied clipboard instead of pretending", async () => {
    clipboardImpl = async () => {
      throw new DOMException("Denied", "NotAllowedError");
    };
    await render();

    await click();

    // The failure path exists in the control (unlike a control with no catch),
    // and it is terminal: there is no manual fallback to offer, so the message
    // says what happened and the cause rides in the tooltip.
    expect(clipboardWrites).toHaveLength(0);
    expect(container.textContent).toBe("Copy HTMLCopy failed.");
    const status = container.querySelector('[role="status"]');
    expect(status?.textContent).toBe("Copy failed.");
    expect(status?.getAttribute("title")).toBe(
      "The browser blocked clipboard access, so nothing was copied.",
    );
  });

  it("retires Copied. after its reading window", async () => {
    vi.useFakeTimers();
    await render();

    await click();
    expect(container.textContent).toContain("Copied.");

    await act(async () => {
      vi.advanceTimersByTime(2_000);
    });

    expect(container.textContent).toBe("Copy HTML");
  });
});
