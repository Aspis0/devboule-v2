// The composer's file chips and routes: picked and dropped files reach the
// file route's callback while images stay on the preview route, a chip is
// removed by its own button, and a send waits for every file to be ready.
// @vitest-environment happy-dom

import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../types/ipc";
import type { AttachedFile } from "./useFileAttachments";
import { WorkspaceComposer } from "./WorkspaceComposer";
import { composerProps } from "./composerTestKit";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

const REFERENCE: AttachmentReference = {
  sessionId: "s.a.1",
  digest: "a".repeat(64),
  storedBytes: 8,
  name: "report.pdf",
};

const READY: AttachedFile = {
  id: "f1",
  name: "report.pdf",
  size: 2 * 1024 * 1024,
  state: "ready",
  reference: REFERENCE,
};

const UPLOADING: AttachedFile = {
  id: "f2",
  name: "notes.txt",
  size: 10,
  state: "uploading",
};

const REFUSED: AttachedFile = {
  id: "f3",
  name: "huge.zip",
  size: 9,
  state: "refused",
  reason: "huge.zip is larger than 50 MiB.",
};

async function render(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
  onSend = vi.fn(async () => true),
) {
  const mocks = { onSend, onQueue: vi.fn() };
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(mocks, overrides)} onSend={onSend} />);
  });
  return onSend;
}

function attachInput(): HTMLInputElement {
  const input = container.querySelector<HTMLInputElement>(
    'input[data-testid="composer-image-input"]',
  );
  if (input === null) throw new Error("attach input did not render");
  return input;
}

async function pickFiles(...files: File[]) {
  const input = attachInput();
  Object.defineProperty(input, "files", { value: files, configurable: true });
  await act(async () => {
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await act(async () => {});
}

function dropFiles(...files: File[]) {
  const composer = container.querySelector<HTMLElement>(".workspace-composer");
  if (composer === null) throw new Error("composer did not render");
  const event = new Event("drop", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", {
    value: { files, types: ["Files"] },
  });
  composer.dispatchEvent(event);
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

async function pressSend() {
  await act(async () => {
    container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

describe("WorkspaceComposer file attachments", () => {
  it("shows one chip per file with its name, size and refusal", async () => {
    await render({ files: [READY, UPLOADING, REFUSED] });
    const chips = container.querySelectorAll('[data-testid="composer-file-chip"]');
    expect(chips).toHaveLength(3);
    expect(chips[0]!.textContent).toContain("report.pdf");
    expect(chips[0]!.textContent).toContain("2.0 MiB");
    expect(chips[1]!.textContent).toContain("Uploading…");
    expect(chips[2]!.textContent).toContain("huge.zip is larger than 50 MiB.");
  });

  it("removes a chip through the parent's callback", async () => {
    const onRemoveFile = vi.fn();
    await render({ files: [READY], onRemoveFile });
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label^="Remove attached file"]')!
        .click();
    });
    expect(onRemoveFile).toHaveBeenCalledWith("f1");
  });

  it("hands a picked text file to the file route without reading it", async () => {
    const onAddFiles = vi.fn();
    const read = vi.spyOn(File.prototype, "arrayBuffer");
    await render({ onAddFiles });
    const notes = new File(["hello"], "notes.txt", { type: "text/plain" });
    await pickFiles(notes);
    expect(onAddFiles).toHaveBeenCalledTimes(1);
    expect(onAddFiles.mock.calls[0]![0]).toEqual([notes]);
    expect(read).not.toHaveBeenCalled();
    expect(container.querySelector('[data-testid="composer-image-preview"]')).toBeNull();
    read.mockRestore();
  });

  it("routes a dropped file to the file route and a dropped image to the previews", async () => {
    const onAddFiles = vi.fn();
    await render({ onAddFiles });
    dropFiles(new File(["data"], "table.csv", { type: "text/csv" }));
    await act(async () => {});
    expect(onAddFiles).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[data-testid="composer-image-preview"]')).toBeNull();

    const png = new File([new Uint8Array([137, 80, 78, 71])], "photo.png", {
      type: "image/png",
    });
    dropFiles(png);
    await act(async () => {});
    expect(container.querySelector('[data-testid="composer-image-preview"]')).not.toBeNull();
    expect(onAddFiles).toHaveBeenCalledTimes(1);
  });

  it("blocks a send while a file is still uploading or refused", async () => {
    const onSend = await render({ files: [UPLOADING] });
    await typeText("look at the notes");
    await pressSend();
    expect(onSend).not.toHaveBeenCalled();
  });

  it("carries the ready references on the send", async () => {
    const onSend = await render({ files: [READY] });
    await typeText("summarise the attached file");
    await pressSend();
    expect(onSend).toHaveBeenCalledTimes(1);
    const [text, attachments, fileReferences] = onSend.mock.calls[0] as [
      string,
      unknown[],
      readonly AttachmentReference[],
    ];
    expect(text).toBe("summarise the attached file");
    expect(attachments).toHaveLength(0);
    expect(fileReferences).toEqual([REFERENCE]);
  });
});
