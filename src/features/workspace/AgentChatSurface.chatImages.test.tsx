// The chat surface's image path: composer images are deposited once, the
// send names the answered references, and the echoed user row shows the
// thumbnails from the stored bytes.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionEvent } from "../../types/ipc";
import type { AttachmentReference } from "../../lib/tauri";

const harness = vi.hoisted(() => ({
  emit: null as ((event: SessionEvent) => void) | null,
  sessionDeposit: vi.fn(),
  sessionSend: vi.fn(),
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
  sessionDeposit: harness.sessionDeposit,
  sessionAttachmentRead: harness.sessionAttachmentRead,
  sessionSend: harness.sessionSend,
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
  sessionId: "chat-images-surface",
  digest: "e".repeat(64),
  storedBytes: 8,
};

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

function textarea(): HTMLTextAreaElement {
  const element = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (element === null) throw new Error("composer textarea did not render");
  return element;
}

async function typeText(text: string) {
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  await act(async () => {
    setValue.call(textarea(), text);
    textarea().dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function pickImage() {
  const input = container.querySelector<HTMLInputElement>(
    'input[data-testid="composer-image-input"]',
  );
  if (input === null) throw new Error("composer image input did not render");
  const file = new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], "photo.png", {
    type: "image/png",
  });
  Object.defineProperty(input, "files", { value: [file], configurable: true });
  await act(async () => {
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await act(async () => {});
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  harness.emit = null;
  harness.sessionDeposit.mockResolvedValue(REF);
  harness.sessionSend.mockResolvedValue(true);
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

describe("chat surface images", () => {
  it("deposits composer images and sends their references, never the bytes", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface daemonState="connected" sessionId="chat-images-surface" />);
    });
    await act(async () => {});
    await pickImage();
    await typeText("look at this");
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    await act(async () => {});

    expect(harness.sessionDeposit).toHaveBeenCalledTimes(1);
    expect(harness.sessionSend).toHaveBeenCalledTimes(1);
    const sendCall = harness.sessionSend.mock.calls[0]!;
    expect(sendCall[2]).toBe("look at this");
    expect(sendCall[3] ?? []).toHaveLength(0);
    expect(sendCall[5]).toEqual([REF]);
  });

  it("shows thumbnails on the echoed user row from the stored bytes", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface daemonState="connected" sessionId="chat-images-surface" />);
    });
    await act(async () => {});
    if (harness.emit === null) throw new Error("surface did not attach");
    await act(async () => {
      harness.emit!({
        type: "agent_user_message",
        messageId: "user-9",
        text: "look at this",
        author: "human",
        messageKind: "composer",
        images: [REF],
      });
    });
    await act(async () => {});
    const thumbnails = container.querySelectorAll('button[aria-label^="Attached image"]');
    expect(thumbnails).toHaveLength(1);
    expect(harness.sessionAttachmentRead).toHaveBeenCalledWith(REF);
  });
});

describe("failed chat image sends", () => {
  async function renderSurface() {
    root = createRoot(container);
    await act(async () => {
      root.render(<AgentChatSurface daemonState="connected" sessionId="chat-images-surface" />);
    });
    await act(async () => {});
  }

  async function pickImageAndType() {
    const input = container.querySelector<HTMLInputElement>(
      'input[data-testid="composer-image-input"]',
    );
    if (input === null) throw new Error("composer image input did not render");
    const file = new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], "photo.png", {
      type: "image/png",
    });
    Object.defineProperty(input, "files", { value: [file], configurable: true });
    await act(async () => {
      input.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => {});
    const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
    const box = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    if (box === null) throw new Error("composer textarea did not render");
    await act(async () => {
      setValue.call(box, "look at this");
      box.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  async function pressSend() {
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    await act(async () => {});
  }

  function preview(): HTMLElement | null {
    return container.querySelector('[data-testid="composer-image-preview"]');
  }

  function draftText(): string {
    return (
      container.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message the agent"]')
        ?.value ?? ""
    );
  }

  it("keeps the picked images and restores the text when the send is refused", async () => {
    harness.sessionSend.mockRejectedValue(new Error("the session refused the prompt"));
    await renderSurface();
    await pickImageAndType();
    await pressSend();
    expect(harness.sessionDeposit).toHaveBeenCalledTimes(1);
    expect(preview()).not.toBeNull();
    expect(draftText()).toBe("look at this");
  });

  it("keeps the picked images when a deposit is refused", async () => {
    harness.sessionDeposit.mockRejectedValue(new Error("too big"));
    await renderSurface();
    await pickImageAndType();
    await pressSend();
    expect(harness.sessionSend).not.toHaveBeenCalled();
    expect(preview()).not.toBeNull();
    expect(draftText()).toBe("look at this");
  });

  it("clears the picked images on success", async () => {
    await renderSurface();
    await pickImageAndType();
    await pressSend();
    expect(preview()).toBeNull();
  });

  it("deposits the same bytes again on retry: two deposits and two sends", async () => {
    harness.sessionSend.mockRejectedValueOnce(new Error("the session refused the prompt"));
    await renderSurface();
    await pickImageAndType();
    await pressSend();
    await pressSend();
    expect(harness.sessionDeposit).toHaveBeenCalledTimes(2);
    expect(harness.sessionSend).toHaveBeenCalledTimes(2);
    expect(harness.sessionDeposit.mock.calls[0]![1]).toEqual(
      harness.sessionDeposit.mock.calls[1]![1],
    );
  });
});
