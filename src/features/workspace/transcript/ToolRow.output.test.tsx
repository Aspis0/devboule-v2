// @vitest-environment happy-dom

// What a tool row shows of its output: nothing on the line itself, the whole of
// it once the person opens the line, a diff coloured as a diff, and a failure's
// words under the line without a click.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../../lib/tauri";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

// The thumbnails read their bytes through the bridge; these cases only need
// the row to hold the images, so the read never resolves.
vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  sessionAttachmentRead: vi.fn(() => new Promise(() => undefined)),
}));

const IMAGE: AttachmentReference = {
  sessionId: "s.owner.chat1",
  digest: "c".repeat(64),
  storedBytes: 12,
};

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

async function rerender(item: ToolChatItem): Promise<void> {
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
}

/** The person opens the line: the disclosure's own toggle, as the browser fires it. */
async function openLine(container: HTMLElement): Promise<HTMLDetailsElement> {
  const details = container.querySelector("details");
  if (details === null) throw new Error("the row had nothing to open");
  await act(async () => {
    details.open = true;
    details.dispatchEvent(new Event("toggle"));
  });
  return details;
}

const lines = (count: number, prefix = "line") =>
  Array.from({ length: count }, (_, index) => `${prefix} ${index + 1}`).join("\n");

const shownLines = (container: HTMLElement): string[] =>
  Array.from(container.querySelectorAll(".workspace-chat-tool-output-line")).map(
    (line) => line.textContent ?? "",
  );

describe("ToolRow output", () => {
  it("is a plain line, with nothing to open, when the call said nothing", async () => {
    const container = await renderRow(tool({ kind: "execute", title: "ls", command: "ls" }));
    expect(container.querySelector("details")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-summary")?.tagName).toBe("DIV");
  });

  it("shows no output on the line, and the whole output once the person opens it", async () => {
    const container = await renderRow(tool({ kind: "search", title: "q", output: lines(10) }));
    expect(shownLines(container)).toEqual([]);

    await openLine(container);

    expect(shownLines(container)).toEqual(
      Array.from({ length: 10 }, (_, index) => `line ${index + 1}`),
    );
    expect(container.querySelector(".workspace-chat-tool-more")).toBeNull();
  });

  it("opens an edit to its diff, with each line coloured by its kind", async () => {
    const diff = ["@@ f", "- old", "+ new", "+ newer", "  same"].join("\n");
    const container = await renderRow(
      tool({ kind: "edit", title: "src/a.ts", locations: [{ path: "src/a.ts" }], output: diff }),
    );
    await openLine(container);
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
    await openLine(container);
    expect(container.querySelector(".workspace-chat-tool-output.is-diff")).toBeNull();
  });

  it("keeps an edit that said no diff as plain output", async () => {
    const container = await renderRow(
      tool({ kind: "edit", title: "src/a.ts", output: "The file was updated." }),
    );
    await openLine(container);
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

  it("keeps a row the person opened open when its output changes", async () => {
    const item = tool({ kind: "search", title: "q", output: lines(10) });
    const container = await renderRow(item);
    await openLine(container);

    await rerender({ ...item, output: lines(11) });

    expect(container.querySelector("details")?.open).toBe(true);
    expect(shownLines(container)).toHaveLength(11);
  });

  it("shows the images of a row once it is opened, when they arrive late", async () => {
    const item = tool({ kind: "search", title: "q", output: lines(10) });
    const container = await renderRow(item);
    await openLine(container);

    await rerender({ ...item, images: [IMAGE] });

    expect(container.querySelector(".workspace-chat-images")).not.toBeNull();
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
    await openLine(container);

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
    await openLine(container);
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

  it("puts an opened output box that can scroll on the keyboard, and leaves a failure excerpt off it", async () => {
    const plain = await renderRow(tool({ kind: "search", title: "q", output: lines(10) }));
    await openLine(plain);
    const box = plain.querySelector(".workspace-chat-tool-output-lines");
    expect(box?.getAttribute("tabindex")).toBe("0");
    expect(box?.getAttribute("aria-label")).toBe("Tool output");
    await act(async () => root?.unmount());
    root = null;
    host.remove();

    const failed = await renderRow(
      tool({
        kind: "execute",
        title: "t",
        command: "t",
        status: "failed",
        output: lines(5, "err"),
      }),
    );
    expect(
      failed.querySelector(".workspace-chat-tool-output-lines")?.hasAttribute("tabindex"),
    ).toBe(false);
  });

  it("says nothing extra for a failure that printed nothing", async () => {
    const container = await renderRow(
      tool({ kind: "execute", title: "t", command: "t", status: "failed", output: "" }),
    );
    expect(container.querySelector(".workspace-chat-tool-output")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-failed")?.textContent).toContain("failed");
  });
});
