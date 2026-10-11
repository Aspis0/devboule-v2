// @vitest-environment happy-dom

// A tool's picture stands under its line, outside the disclosure, so a closed line still shows it.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../../lib/tauri";
import type { ToolChatItem } from "../../../lib/toolCallGroups";

const readMocks = vi.hoisted(() => ({ sessionAttachmentRead: vi.fn() }));

vi.mock("../../../lib/tauri", () => ({
  sessionAttachmentRead: readMocks.sessionAttachmentRead,
}));

import { resetChatImageCacheForTests } from "./chatImageCache";
import { ToolRow } from "./ToolRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const IMAGE: AttachmentReference = {
  sessionId: "s.child.1",
  digest: "c".repeat(64),
  storedBytes: 12,
};

let container: HTMLDivElement;
let root: Root | null = null;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  readMocks.sessionAttachmentRead.mockImplementation(async () => ({
    mimeType: "image/png",
    data: "aGk=",
  }));
  vi.stubGlobal("IntersectionObserver", undefined);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  container.remove();
  resetChatImageCacheForTests();
  vi.unstubAllGlobals();
});

function tool(title: string, images: AttachmentReference[]): ToolChatItem {
  return {
    id: "tool-1",
    role: "tool",
    title,
    output: "",
    toolCallId: "tool-1",
    status: "completed",
    kind: "other",
    images,
  };
}

async function show(item: ToolChatItem): Promise<void> {
  root = createRoot(container);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  await act(async () => {});
}

describe("a tool's picture", () => {
  it("stands under the line of a file-producing tool, outside the disclosure", async () => {
    await show(tool("Render mockup", [IMAGE]));

    const picture = container.querySelector(".workspace-chat-tool-picture");
    expect(picture).not.toBeNull();
    expect(picture?.closest("details")).toBeNull();
    expect(picture?.querySelector("button")?.getAttribute("aria-label")).toBe("Image 1 of 1");
  });

  it("draws no picture for a browser call that is not a screenshot", async () => {
    await show(tool("browser_click", [IMAGE]));

    expect(container.querySelector(".workspace-chat-tool-picture")).toBeNull();
  });
});
