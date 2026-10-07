// Provider-produced images on the transcript: a generated assistant image and
// a tool-result image both render as thumbnails from the stored bytes.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";
import type { AttachmentReference } from "../../lib/tauri";

const harness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  sessionAttachmentRead: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: vi.fn(async () => []),
  createSessionChannel: vi.fn((onEvent: (event: SessionEvent) => void) => {
    harness.emit = onEvent;
    return {};
  }),
  sessionAttach: vi.fn(async () => {
    await Promise.resolve();
    return 41;
  }),
  sessionDetach: vi.fn(async () => undefined),
  sessionAttachmentRead: harness.sessionAttachmentRead,
  sessionInterrupt: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  sessionSetFeature: vi.fn(async () => undefined),
  isCommandError: () => false,
}));

import { AgentChatSurface } from "./AgentChatSurface";
import { resetChatImageCacheForTests } from "./transcript/chatImageCache";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const REF: AttachmentReference = {
  sessionId: "agent-images-surface",
  digest: "a".repeat(64),
  storedBytes: 8,
};

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

async function mount() {
  root = createRoot(container);
  await act(async () => {
    root.render(<AgentChatSurface daemonState="connected" sessionId="agent-images-surface" />);
  });
  await act(async () => {});
}

async function emit(event: SessionEvent) {
  await act(async () => {
    harness.emit?.(event);
  });
  await act(async () => {});
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  harness.emit = null;
  harness.sessionAttachmentRead.mockResolvedValue({ mimeType: "image/png", data: "aGk=" });
  vi.stubGlobal("IntersectionObserver", undefined);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  resetChatImageCacheForTests();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("agent-produced images", () => {
  it("renders a generated assistant image and a tool-result image as thumbnails", async () => {
    await mount();
    await emit({
      type: "agent_message",
      messageId: "img-1",
      text: "",
      images: [REF],
    });
    await emit({
      type: "agent_tool_update",
      toolCallId: "call-1",
      status: "completed",
      text: "[image]",
      title: "mcp__devboule__browser_screenshot",
      images: [REF],
    });

    const thumbs = container.querySelectorAll(".workspace-chat-image-thumb");
    expect(thumbs).toHaveLength(2);
    expect(
      container.querySelector(".workspace-chat-tool-body .workspace-chat-image-thumb"),
    ).not.toBeNull();
    // The image row is open, so the image is not behind a collapsed triangle.
    const details = container.querySelector<HTMLDetailsElement>("details");
    expect(details?.open).toBe(true);
    // A person's collapse stands across the next re-render of the row.
    if (details !== null) details.open = false;
    await emit({
      type: "agent_tool_update",
      toolCallId: "call-1",
      status: "completed",
      text: "[image] and more",
      title: "mcp__devboule__browser_screenshot",
      images: [REF],
    });
    expect(container.querySelector<HTMLDetailsElement>("details")?.open).toBe(false);
    // An image-only assistant message renders no empty copy block.
    expect(container.querySelectorAll(".workspace-chat-copy")).toHaveLength(0);
    expect(harness.sessionAttachmentRead).toHaveBeenCalledWith(REF);
  });
});
