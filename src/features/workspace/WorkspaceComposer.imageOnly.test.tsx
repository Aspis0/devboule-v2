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
});

describe("WorkspaceComposer image-only queue", () => {
  const queueOptions = { turnActive: true, enterQueues: true } as const;

  it("keeps queue off for an image with no text when the target cannot take images", async () => {
    await renderComposer(vi.fn(), { ...queueOptions, imageOnlyAccepted: false });
    await pickImage();
    expect(queueButton().disabled).toBe(true);
  });
});
