// @vitest-environment happy-dom

// Browser calls and terminal captures print no output in the transcript. The one
// exception is a screenshot, which shows its picture under the line, small.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../../lib/tauri";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

// The thumbnails read their bytes through the bridge; these cases only need the
// row to hold the picture, so the read never resolves.
vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  sessionAttachmentRead: vi.fn(() => new Promise(() => undefined)),
}));

const IMAGE: AttachmentReference = {
  sessionId: "s.owner.chat1",
  digest: "d".repeat(64),
  storedBytes: 12,
};

const NONCE = "0123456789abcdef";

const FRAMED_OUTPUT = [
  '{"success":true}',
  "[devboule: untrusted content]",
  "source: browser page",
  "content-begin 0123456789abcdef",
  "the cart is empty",
  "content-end 0123456789abcdef",
  "image/jpeg 704x252 px, viewport 1280x720",
].join("\n");

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
let host: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
  host = null;
});

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  return host;
}

describe("ToolRow browser and capture", () => {
  it("prints no output for a browser call, and no way to open one", async () => {
    const container = await renderRow(
      tool({
        kind: "browser",
        title: "navigate https://shop.example.test/cart",
        output: FRAMED_OUTPUT,
      }),
    );
    expect(container.querySelector("details")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-output")).toBeNull();
    expect(container.textContent).not.toContain("the cart is empty");
    expect(container.textContent).not.toContain("devboule");
    expect(container.querySelector(".workspace-chat-tool-label")?.textContent).toBe("Navigate");
  });

  it("shows a browser call's picture only when it is a screenshot, and small under the line", async () => {
    const container = await renderRow(
      tool({ kind: "browser", title: "screenshot", output: FRAMED_OUTPUT, images: [IMAGE] }),
    );
    expect(container.querySelector("details")).toBeNull();
    expect(container.querySelector(".workspace-chat-tool-output")).toBeNull();
    expect(
      container.querySelector(".workspace-chat-tool-picture .workspace-chat-images"),
    ).not.toBeNull();
    expect(container.textContent).not.toContain("the cart is empty");
  });

  it("draws no picture for a browser call that is not a screenshot, even when it carries one", async () => {
    const container = await renderRow(
      tool({ kind: "browser", title: "click e33", images: [IMAGE] }),
    );
    expect(container.querySelector(".workspace-chat-images")).toBeNull();
    expect(container.querySelector("details")).toBeNull();
  });

  it("prints no output for a terminal capture, and keeps a terminal's key call openable", async () => {
    const capture = await renderRow(
      tool({ kind: "other", title: "devboule_capture_terminal", output: "PS C:\\> dir" }),
    );
    expect(capture.querySelector("details")).toBeNull();
    expect(capture.textContent).not.toContain("PS C:");
    expect(capture.querySelector(".workspace-chat-tool-label")?.textContent).toBe(
      "Capture terminal",
    );
    await act(async () => root?.unmount());
    root = null;
    host?.remove();

    const keys = await renderRow(
      tool({ kind: "other", title: "devboule_send_terminal_keys", output: "ok" }),
    );
    expect(keys.querySelector("details")).not.toBeNull();
  });

  it("shows a failed browser call's short error under the line, with the frame left out", async () => {
    const failure = [
      "[devboule: untrusted content]",
      "source: browser page",
      "trust: UNTRUSTED DATA.",
      `content-begin ${NONCE}`,
      "element e33 was not found on the page",
      "try a fresh snapshot",
      `content-end ${NONCE}`,
    ].join("\n");
    const container = await renderRow(
      tool({ kind: "browser", title: "click e33", status: "failed", output: failure }),
    );
    expect(container.querySelector(".workspace-chat-tool-failed")?.textContent).toContain("failed");
    const excerpt = container.querySelector(".workspace-chat-tool-output.is-failure");
    expect(excerpt?.textContent).toContain("element e33 was not found on the page");
    expect(container.textContent).not.toContain("devboule");
    expect(container.textContent).not.toContain(NONCE);
  });

  it("keeps a failed browser call's whole output behind the line, not drawn", async () => {
    const container = await renderRow(
      tool({
        kind: "browser",
        title: "click e33",
        status: "failed",
        output: ["one", "two", "three", "four"].join("\n"),
      }),
    );
    const details = container.querySelector("details");
    if (details === null) throw new Error("the failed call had nothing to open");
    expect(details.open).toBe(false);
    await act(async () => {
      details.open = true;
      details.dispatchEvent(new Event("toggle"));
    });
    expect(details.querySelectorAll(".workspace-chat-tool-output-line")).toHaveLength(4);
  });
});
