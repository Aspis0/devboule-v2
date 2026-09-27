// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MessageCopyButton } from "./MessageCopyButton";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("timeline message copy button", () => {
  let container: HTMLDivElement;
  let root: Root;
  let writeText: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.restoreAllMocks();
  });

  it("is keyboard reachable and copies the unformatted message text", async () => {
    const raw = "**bold**\nC:\\Users\\x\\design-sandbox";
    await act(async () => root.render(<MessageCopyButton text={raw} />));

    const button = container.querySelector("button");
    expect(button?.type).toBe("button");
    expect(button?.tabIndex).toBe(0);
    expect(button?.getAttribute("aria-label")).toBe("Copy message");
    await act(async () => button?.click());

    expect(writeText).toHaveBeenCalledWith(raw);
    expect(button?.getAttribute("aria-label")).toBe("Copied");
  });

  it("announces clipboard failures without throwing", async () => {
    writeText.mockRejectedValueOnce(new Error("denied"));
    await act(async () => root.render(<MessageCopyButton text="message" />));
    const button = container.querySelector("button");

    await act(async () => button?.click());

    expect(button?.getAttribute("aria-label")).toBe("Copy failed");
  });
});
