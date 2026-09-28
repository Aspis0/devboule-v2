// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CodeBlock } from "./CodeBlock";
import { MarkdownText } from "./MarkdownText";
import { MessageCopyButton } from "../features/workspace/timeline/MessageCopyButton";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("fenced block copy", () => {
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
    vi.useRealTimers();
  });

  it("copies the exact raw text of a copyable block, fences excluded", async () => {
    await act(async () =>
      root.render(<CodeBlock code="pnpm vitest checkout --profile staging" copyable />),
    );

    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    expect(button?.getAttribute("aria-label")).toBe("Copy code");
    await act(async () => button?.click());

    expect(writeText).toHaveBeenCalledWith("pnpm vitest checkout --profile staging");
    expect(button?.getAttribute("aria-label")).toBe("Copied");
  });

  it("gives a code sample the same copy button over its dark body", async () => {
    await act(async () =>
      root.render(<CodeBlock code={"const a = 1;\nconst b = 2;"} copyable={false} />),
    );

    const sample = container.querySelector(".codeblock-sample");
    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    expect(sample?.querySelector("pre code")?.textContent).toBe("const a = 1;\nconst b = 2;");
    expect(button).not.toBeNull();
    await act(async () => button?.click());

    expect(writeText).toHaveBeenCalledWith("const a = 1;\nconst b = 2;");
  });

  it("announces the copy in a live region that pre-existed the copy", async () => {
    await act(async () => root.render(<CodeBlock code="pnpm build" copyable />));

    const region = container.querySelector('[aria-live="polite"]');
    expect(region).not.toBeNull();
    expect(region?.textContent).toBe("");
    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    await act(async () => button?.click());

    expect(container.querySelector('[aria-live="polite"]')).toBe(region);
    expect(region?.textContent).toBe("Copied");
  });

  it("keeps Copied past the 1.5 s reset the message chip still uses", async () => {
    vi.useFakeTimers();
    try {
      await act(async () =>
        root.render(
          <div className="workspace-chat-assistant">
            <div className="workspace-chat-copy">
              <MarkdownText text={"```bash\npnpm build"} />
            </div>
            <MessageCopyButton text="the message" />
          </div>,
        ),
      );
      const chip = container.querySelector<HTMLButtonElement>(".timeline-copy-chip");
      const button = container.querySelector<HTMLButtonElement>(".copy-btn");
      await act(async () => button?.click());

      expect(button?.getAttribute("aria-label")).toBe("Copied");
      await act(async () => vi.advanceTimersByTime(1600));
      expect(button?.getAttribute("aria-label")).toBe("Copied");
      expect(chip?.getAttribute("aria-label")).toBe("Copy message");
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not claim Copied when the clipboard fails", async () => {
    writeText.mockRejectedValueOnce(new Error("denied"));
    await act(async () => root.render(<CodeBlock code="pnpm build" copyable />));

    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    await act(async () => button?.click());

    expect(button?.getAttribute("aria-label")).toBe("Copy code");
    expect(button?.textContent).toBe("Copy");
  });

  it("drops a blank line the author left before the closing fence", async () => {
    await act(async () => root.render(<CodeBlock code={"pnpm build\n"} copyable />));

    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    await act(async () => button?.click());

    expect(writeText).toHaveBeenCalledWith("pnpm build");
  });

  it("keeps the message line endings instead of normalising them", async () => {
    await act(async () => root.render(<CodeBlock code={"pnpm build\r\npnpm test"} copyable />));

    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    await act(async () => button?.click());

    expect(writeText).toHaveBeenCalledWith("pnpm build\r\npnpm test");
  });

  it("copies what is there when the fence is still streaming", async () => {
    await act(async () => root.render(<CodeBlock code="pnpm install" copyable />));

    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    expect(button).not.toBeNull();
    await act(async () => button?.click());

    expect(writeText).toHaveBeenCalledWith("pnpm install");
  });

  it("is keyboard reachable", async () => {
    await act(async () => root.render(<CodeBlock code="pnpm build" copyable />));

    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    expect(button?.tabIndex).toBe(0);
  });

  it("renders nothing at all for an empty fence", async () => {
    await act(async () => root.render(<CodeBlock code="" copyable />));

    expect(container.querySelector(".copyblock")).toBeNull();
    expect(container.querySelector(".copy-btn")).toBeNull();
  });

  it("keeps Copied when a streaming block crosses the copyable threshold", async () => {
    await act(async () => root.render(<MarkdownText text={"```\nline one\nline two"} />));
    const button = container.querySelector<HTMLButtonElement>(".copy-btn");
    await act(async () => button?.click());
    expect(button?.getAttribute("aria-label")).toBe("Copied");

    await act(async () =>
      root.render(<MarkdownText text={"```\nline one\nline two\nline three\nline four"} />),
    );
    const flipped = container.querySelector<HTMLButtonElement>(".copy-btn");
    expect(flipped?.getAttribute("aria-label")).toBe("Copied");
    expect(container.querySelector(".codeblock-sample")).not.toBeNull();
  });

  it("a code-block copy leaves the message copy beside it untouched", async () => {
    await act(async () =>
      root.render(
        <div className="workspace-chat-assistant">
          <div className="workspace-chat-copy">
            <MarkdownText text={"```bash\npnpm build"} />
          </div>
          <MessageCopyButton text="the message" />
        </div>,
      ),
    );
    const chip = container.querySelector<HTMLButtonElement>(".timeline-copy-chip");
    const button = container.querySelector<HTMLButtonElement>(".copy-btn");

    await act(async () => button?.click());
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith("pnpm build");
    expect(chip?.getAttribute("aria-label")).toBe("Copy message");

    await act(async () => chip?.click());
    expect(writeText).toHaveBeenCalledTimes(2);
    expect(writeText).toHaveBeenLastCalledWith("the message");
    expect(chip?.getAttribute("aria-label")).toBe("Copied");
  });
});
