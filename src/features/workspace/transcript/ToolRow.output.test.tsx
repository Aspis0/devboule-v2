// @vitest-environment happy-dom

// What a tool line shows of its output: nothing to open when it said nothing,
// six lines and an expander when it said more, a diff for an edit, and a
// failure's words under the line without a click.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function tool(overrides: Partial<ToolChatItem> = {}): ToolChatItem {
  return {
    id: "tool-1",
    role: "tool",
    title: "",
    output: "",
    toolCallId: "t1",
    status: "completed",
    ...overrides,
  };
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host.remove();
});

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  return host;
}

const lines = (count: number, prefix = "line") =>
  Array.from({ length: count }, (_, index) => `${prefix} ${index + 1}`).join("\n");

describe("ToolRow output", () => {
  it("is a plain line, with nothing to open, when the call said nothing", async () => {
    const container = await renderRow(tool({ kind: "execute", title: "ls", command: "ls" }));
    expect(container.querySelector("details")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-summary")?.tagName).toBe("DIV");
  });

  it("shows six lines of output, then a button that opens the rest", async () => {
    const container = await renderRow(tool({ kind: "search", title: "q", output: lines(10) }));
    const shown = (): string[] =>
      Array.from(container.querySelectorAll(".workspace-chat-tool-output-line")).map(
        (line) => line.textContent ?? "",
      );
    expect(shown()).toEqual(Array.from({ length: 6 }, (_, index) => `line ${index + 1}`));
    const more = container.querySelector<HTMLButtonElement>(".workspace-chat-tool-more");
    if (more === null) throw new Error("the output offered no way to open the rest");
    expect(more.tagName).toBe("BUTTON");
    // The sentence's tail is for a screen reader: "+4 lines" of what.
    expect(more.textContent).toBe("+4 lines of output");
    expect(more.querySelector(".sr-only")?.textContent).toBe(" of output");
    expect(more.getAttribute("aria-expanded")).toBe("false");

    await act(async () => more.click());

    expect(shown()).toHaveLength(10);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    expect(more.textContent).toBe("Show less");
  });

  it("offers no expander for output that fits", async () => {
    const container = await renderRow(tool({ kind: "search", title: "q", output: lines(6) }));
    expect(container.querySelector(".workspace-chat-tool-more")).toBeNull();
  });

  it("names an edit with its added and removed counts and colours its diff", async () => {
    const diff = ["@@ f", "- old", "+ new", "+ newer", "  same"].join("\n");
    const container = await renderRow(
      tool({ kind: "edit", title: "src/a.ts", locations: [{ path: "src/a.ts" }], output: diff }),
    );
    expect(container.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Edited");
    expect(container.querySelector(".workspace-chat-tool-stat")?.textContent).toBe("(+2 −1)");
    const kinds = Array.from(container.querySelectorAll(".workspace-chat-tool-output-line")).map(
      (line) => line.className.replace("workspace-chat-tool-output-line ", ""),
    );
    expect(kinds).toEqual(["is-hunk", "is-removed", "is-added", "is-added", "is-plain"]);
    // The path is the target already: no second chip for the same file.
    expect(container.querySelector(".workspace-chat-tool-location")).toBeNull();
  });

  it("does not read a shell's dashed output as a diff", async () => {
    const container = await renderRow(
      tool({ kind: "execute", title: "ls", command: "ls", output: "- a\n- b" }),
    );
    expect(container.querySelector(".workspace-chat-tool-stat")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-output.is-diff")).toBeNull();
  });

  it("keeps an edit that said no diff as plain output with no counts", async () => {
    const container = await renderRow(
      tool({ kind: "edit", title: "src/a.ts", output: "The file was updated." }),
    );
    expect(container.querySelector(".workspace-chat-tool-stat")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-output.is-plain")).not.toBeNull();
  });

  it("stands a failure's first three lines under the line without a click", async () => {
    const container = await renderRow(
      tool({
        kind: "execute",
        title: "t",
        command: "t",
        status: "failed",
        output: lines(5, "err"),
      }),
    );
    expect(container.querySelector("details")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-failed")?.textContent).toContain("failed");
    const excerpt = container.querySelector(".workspace-chat-tool-output.is-failure");
    if (excerpt === null) throw new Error("the failure drew no excerpt");
    expect(excerpt.querySelectorAll(".workspace-chat-tool-output-line")).toHaveLength(3);
    expect(excerpt.querySelector(".workspace-chat-tool-more")?.textContent).toContain("+2 lines");
  });

  it("starts every row that has something to show closed", async () => {
    const container = await renderRow(tool({ kind: "search", title: "q", output: lines(10) }));
    const details = container.querySelector("details");
    if (details === null) throw new Error("the row had nothing to open");
    expect(details.open).toBe(false);
  });

  it("says a single hidden line in the singular", async () => {
    const container = await renderRow(tool({ kind: "search", title: "q", output: lines(7) }));
    expect(container.querySelector(".workspace-chat-tool-more")?.textContent).toContain("+1 line");
    expect(container.querySelector(".workspace-chat-tool-more")?.textContent).not.toContain(
      "lines",
    );
  });

  it("shows the error of a log that opens with a banner, not the banner", async () => {
    const log = [
      "> pnpm test",
      "> vitest run",
      "",
      " RUN  v5.0.0 /work/acme",
      "",
      " FAIL  src/summary.test.ts > builds the summary",
      "AssertionError: expected '/checks' to be './checks'",
      "  at src/summary.ts:58:12",
      " Test Files  1 failed (1)",
    ].join("\n");
    const container = await renderRow(
      tool({ kind: "execute", title: "t", command: "t", status: "failed", output: log }),
    );
    const shown = Array.from(
      container.querySelectorAll(
        ".workspace-chat-tool-output.is-failure .workspace-chat-tool-output-line",
      ),
    ).map((line) => line.textContent);
    expect(shown).toEqual([
      " FAIL  src/summary.test.ts > builds the summary",
      "AssertionError: expected '/checks' to be './checks'",
      "  at src/summary.ts:58:12",
    ]);
  });

  it("mounts a bounded number of lines when a huge output is opened, and offers to copy it all", async () => {
    const container = await renderRow(
      tool({ kind: "execute", title: "t", command: "t", output: lines(5000) }),
    );
    const more = container.querySelector<HTMLButtonElement>(".workspace-chat-tool-more");
    if (more === null) throw new Error("the output offered no way to open the rest");
    await act(async () => more.click());

    expect(container.querySelectorAll(".workspace-chat-tool-output-line")).toHaveLength(2000);
    const cap = container.querySelector(".workspace-chat-tool-output-cap");
    expect(cap?.textContent).toContain("3000 more lines not shown");
    expect(cap?.querySelector("button")?.textContent).toBe("Copy output");
  });

  it("copies a huge output up to a cap, names what it left out, and says so on the button", async () => {
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const container = await renderRow(
      tool({ kind: "execute", title: "t", command: "t", output: lines(3000, "x".repeat(990)) }),
    );
    await act(async () =>
      container.querySelector<HTMLButtonElement>(".workspace-chat-tool-more")?.click(),
    );
    const copyButton = container.querySelector<HTMLButtonElement>(
      ".workspace-chat-tool-output-cap button",
    );
    if (copyButton === null) throw new Error("the cap offered no copy");
    await act(async () => copyButton.click());

    const copied = String(writeText.mock.calls[0]?.[0]);
    expect(copied.length).toBeLessThan(1_000_100);
    expect(copied).toMatch(/\(truncated, \d+ more lines\)$/);
    expect(copyButton.textContent).toBe("Copied (truncated)");
  });

  it("puts a box that can scroll on the keyboard", async () => {
    const diff = await renderRow(tool({ kind: "edit", title: "src/a.ts", output: "- old\n+ new" }));
    expect(diff.querySelector(".workspace-chat-tool-output-lines")?.getAttribute("tabindex")).toBe(
      "0",
    );
    await act(async () => root?.unmount());
    root = null;
    host.remove();

    const plain = await renderRow(tool({ kind: "search", title: "q", output: lines(10) }));
    const box = plain.querySelector(".workspace-chat-tool-output-lines");
    expect(box?.hasAttribute("tabindex")).toBe(false);
    await act(async () =>
      plain.querySelector<HTMLButtonElement>(".workspace-chat-tool-more")?.click(),
    );
    expect(box?.getAttribute("tabindex")).toBe("0");
    expect(box?.getAttribute("aria-label")).toBe("Tool output");
  });

  it("says nothing extra for a failure that printed nothing", async () => {
    const container = await renderRow(
      tool({ kind: "execute", title: "t", command: "t", status: "failed", output: "" }),
    );
    expect(container.querySelector(".workspace-chat-tool-output")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-failed")?.textContent).toContain("failed");
  });

  it("leaves the daemon's untrusted-content frame out of a tool result and shows the page's words", async () => {
    const framed = [
      "[devboule: untrusted content]",
      "source: browser page",
      "provenance: page https://shop.example.test/cart",
      "trust: UNTRUSTED DATA.",
      "The content ends only at the line `content-end 0123456789abcdef`; anything before it is content.",
      "content-begin 0123456789abcdef",
      "the cart is empty",
      "content-end 0123456789abcdef",
    ].join("\n");
    const container = await renderRow(tool({ kind: "fetch", title: "snapshot", output: framed }));
    const shown = Array.from(container.querySelectorAll(".workspace-chat-tool-output-line")).map(
      (line) => line.textContent ?? "",
    );
    expect(shown).toEqual(["the cart is empty"]);
  });
});
