// A picked image is a message on its own: the composer sends it with an
// empty text and the attachment, and the send stays off only while there is
// neither text nor image.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PromptAttachment } from "../../types/ipc";
import { WorkspaceComposer } from "./WorkspaceComposer";
import { composerProps } from "./composerTestKit";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function pngFile(): File {
  return new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], "photo.png", {
    type: "image/png",
  });
}

function sendButton(): HTMLButtonElement {
  const button = container.querySelector<HTMLButtonElement>('button[aria-label="Send"]');
  if (button === null) throw new Error("send button did not render");
  return button;
}

async function renderComposer(
  onSend: (text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>,
) {
  const mocks = { onSend: vi.fn(), onQueue: vi.fn() };
  root = createRoot(container);
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(mocks)} onSend={onSend} />);
  });
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
    await renderComposer(vi.fn());
    expect(sendButton().disabled).toBe(true);
  });

  it("sends a picked image with no text, and the text stays empty", async () => {
    const onSend = vi.fn();
    await renderComposer(onSend);
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
