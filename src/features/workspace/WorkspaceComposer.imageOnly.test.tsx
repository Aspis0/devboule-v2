// A picked image is a message on its own only where the target takes images:
// the composer sends or queues it with an empty text, and the send stays off
// for a target that cannot read images, as it did before. Text plus image is
// unaffected by the target's capability.
// @vitest-environment happy-dom

import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PromptAttachment } from "../../types/ipc";
import { WorkspaceComposer } from "./WorkspaceComposer";
import { composerProps } from "./composerTestKit";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type SendMock = (text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>;

let container: HTMLDivElement;
let root: Root;

function pngFile(): File {
  return new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], "photo.png", {
    type: "image/png",
  });
}

function button(selector: string): HTMLButtonElement {
  const element = container.querySelector<HTMLButtonElement>(selector);
  if (element === null) throw new Error(`${selector} did not render`);
  return element;
}

function sendButton(): HTMLButtonElement {
  return button('button[aria-label="Send"]');
}

function queueButton(): HTMLButtonElement {
  return button('button[data-testid="composer-queue-action"]');
}

async function renderComposer(
  onSend: (text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>,
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
) {
  const mocks = { onSend: vi.fn(), onQueue: vi.fn() };
  root = createRoot(container);
  await act(async () => {
    root.render(
      <WorkspaceComposer
        {...composerProps(mocks, overrides)}
        onSend={onSend}
        onQueue={mocks.onQueue}
      />,
    );
  });
  return mocks;
}

async function pickImage() {
  const input = container.querySelector<HTMLInputElement>(
    'input[data-testid="composer-image-input"]',
  );
  if (input === null) throw new Error("composer image input did not render");
  Object.defineProperty(input, "files", { value: [pngFile()], configurable: true });
  await act(async () => {
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await act(async () => {});
}

async function typeText(text: string) {
  const textarea = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (textarea === null) throw new Error("composer textarea did not render");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  await act(async () => {
    setValue.call(textarea, text);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("WorkspaceComposer image-only sends", () => {
  it("keeps send off while there is neither text nor image", async () => {
    await renderComposer(vi.fn(), { imageOnlyAccepted: true });
    expect(sendButton().disabled).toBe(true);
  });

  it("keeps send off for an image with no text when the target cannot take images", async () => {
    await renderComposer(vi.fn(), { imageOnlyAccepted: false });
    await pickImage();
    expect(sendButton().disabled).toBe(true);
  });

  it("sends a picked image with no text when the target takes images", async () => {
    const onSend = vi.fn<SendMock>(async () => true);
    await renderComposer(onSend, { imageOnlyAccepted: true });
    await pickImage();
    expect(sendButton().disabled).toBe(false);
    await act(async () => {
      sendButton().click();
    });
    expect(onSend).toHaveBeenCalledTimes(1);
    const [text, attachments] = onSend.mock.calls[0] as [string, readonly PromptAttachment[]];
    expect(text).toBe("");
    expect(attachments.map((attachment) => attachment.mimeType)).toEqual(["image/png"]);
  });

  it("treats whitespace-only text as no text", async () => {
    const onSend = vi.fn<SendMock>(async () => true);
    await renderComposer(onSend, { imageOnlyAccepted: true });
    await pickImage();
    await typeText("   ");
    await act(async () => {
      sendButton().click();
    });
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend.mock.calls[0]?.[0]).toBe("");
  });

  it("disables send again once the last picked image is removed", async () => {
    await renderComposer(vi.fn(), { imageOnlyAccepted: true });
    await pickImage();
    expect(sendButton().disabled).toBe(false);
    await act(async () => {
      button('button[aria-label^="Remove attached image"]').click();
    });
    expect(sendButton().disabled).toBe(true);
  });
});

describe("WorkspaceComposer image-only queue", () => {
  const queueOptions = { turnActive: true, enterQueues: true } as const;

  it("keeps queue off for an image with no text when the target cannot take images", async () => {
    await renderComposer(vi.fn(), { ...queueOptions, imageOnlyAccepted: false });
    await pickImage();
    expect(queueButton().disabled).toBe(true);
  });

  it("queues a picked image with no text when the target takes images", async () => {
    const mocks = await renderComposer(vi.fn(), { ...queueOptions, imageOnlyAccepted: true });
    await pickImage();
    expect(queueButton().disabled).toBe(false);
    await act(async () => {
      queueButton().click();
    });
    expect(mocks.onQueue).toHaveBeenCalledTimes(1);
    const [text, attachments] = mocks.onQueue.mock.calls[0] as [
      string,
      readonly PromptAttachment[],
    ];
    expect(text).toBe("");
    expect(attachments.map((attachment) => attachment.mimeType)).toEqual(["image/png"]);
  });

  it("disables queue again once the last picked image is removed", async () => {
    await renderComposer(vi.fn(), { ...queueOptions, imageOnlyAccepted: true });
    await pickImage();
    await act(async () => {
      button('button[aria-label^="Remove attached image"]').click();
    });
    expect(queueButton().disabled).toBe(true);
  });
});

describe("WorkspaceComposer pasted images", () => {
  function paste(files: File[]): Event {
    const event = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { files, types: ["Files"] } });
    return event;
  }

  function textarea(): HTMLTextAreaElement {
    const element = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    if (element === null) throw new Error("composer textarea did not render");
    return element;
  }

  it("attaches a pasted image, and removing it closes the send again", async () => {
    const onSend = vi.fn();
    await renderComposer(onSend, { imageOnlyAccepted: true });
    const event = paste([pngFile()]);
    await act(async () => {
      textarea().dispatchEvent(event);
    });
    await act(async () => {});
    expect(event.defaultPrevented).toBe(true);
    expect(container.querySelector('[data-testid="composer-image-preview"]')).not.toBeNull();
    expect(sendButton().disabled).toBe(false);
    await act(async () => {
      button('button[aria-label^="Remove attached image"]').click();
    });
    expect(container.querySelector('[data-testid="composer-image-preview"]')).toBeNull();
    expect(sendButton().disabled).toBe(true);
  });

  it("leaves a text paste to the textarea", async () => {
    await renderComposer(vi.fn(), { imageOnlyAccepted: true });
    const event = paste([]);
    await act(async () => {
      textarea().dispatchEvent(event);
    });
    expect(event.defaultPrevented).toBe(false);
  });
});
