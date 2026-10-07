// The chat surface's file path: the composer picks a file, the upload frames
// carry it, and the answered reference rides the send; a refusal keeps the
// chip, a success clears it, and removing a ready chip releases the bytes.
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
  sessionAttachmentDelete: vi.fn(),
  sessionQueueAdd: vi.fn(),
  sessionUploadBegin: vi.fn(),
  sessionUploadStatus: vi.fn(),
  sessionUploadChunk: vi.fn(),
  sessionUploadFinish: vi.fn(),
  sessionUploadAbort: vi.fn(),
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
  sessionAttachmentDelete: harness.sessionAttachmentDelete,
  sessionSend: harness.sessionSend,
  sessionQueueAdd: harness.sessionQueueAdd,
  sessionQueueEdit: vi.fn(async () => undefined),
  sessionQueueRemove: vi.fn(async () => undefined),
  sessionQueueMove: vi.fn(async () => undefined),
  sessionQueueSendNow: vi.fn(async () => undefined),
  sessionUploadBegin: harness.sessionUploadBegin,
  sessionUploadStatus: harness.sessionUploadStatus,
  sessionUploadChunk: harness.sessionUploadChunk,
  sessionUploadFinish: harness.sessionUploadFinish,
  sessionUploadAbort: harness.sessionUploadAbort,
  sessionInterrupt: vi.fn(async () => undefined),
  sessionSetModel: vi.fn(async () => undefined),
  sessionSetMode: vi.fn(async () => undefined),
  sessionSetFeature: vi.fn(async () => undefined),
  isCommandError: () => false,
}));

import { AgentChatSurface } from "./AgentChatSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SESSION = "chat-files-surface";
const IMAGE_REF: AttachmentReference = {
  sessionId: SESSION,
  digest: "e".repeat(64),
  storedBytes: 8,
};
const FILE_REF: AttachmentReference = {
  sessionId: SESSION,
  digest: "f".repeat(64),
  storedBytes: 12,
  name: "report.pdf",
};

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

async function renderSurface(
  props: { activity?: "idle" | "working"; queueSupported?: boolean } = {},
) {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <AgentChatSurface
        daemonState="connected"
        sessionId={SESSION}
        activity={props.activity ?? "idle"}
        queueSupported={props.queueSupported ?? false}
        fileUploadSupported
      />,
    );
  });
  await act(async () => {});
}

async function typeText(text: string) {
  const box = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (box === null) throw new Error("composer textarea did not render");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  await act(async () => {
    setValue.call(box, text);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function settleUploads() {
  for (let attempt = 0; attempt < 40 && !chipReady(); attempt += 1) {
    await act(async () => {});
  }
}

async function pickFile(name = "report.pdf") {
  const input = container.querySelector<HTMLInputElement>(
    'input[data-testid="composer-image-input"]',
  );
  if (input === null) throw new Error("composer attach input did not render");
  const file = new File([new Uint8Array([1, 2, 3, 4])], name, { type: "application/pdf" });
  Object.defineProperty(input, "files", { value: [file], configurable: true });
  await act(async () => {
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await settleUploads();
}

async function pickImage() {
  const input = container.querySelector<HTMLInputElement>(
    'input[data-testid="composer-image-input"]',
  );
  if (input === null) throw new Error("composer attach input did not render");
  const file = new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], "photo.png", {
    type: "image/png",
  });
  Object.defineProperty(input, "files", { value: [file], configurable: true });
  await act(async () => {
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await act(async () => {});
}

async function pressSend() {
  await act(async () => {
    container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
  });
  await act(async () => {});
  await act(async () => {});
}

function chipReady(): boolean {
  const chip = container.querySelector('[data-testid="composer-file-chip"]');
  return chip !== null && !chip.textContent?.includes("Uploading");
}

function chipText(): string {
  return container.querySelector('[data-testid="composer-file-chip"]')?.textContent ?? "";
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  harness.emit = null;
  harness.sessionDeposit.mockResolvedValue(IMAGE_REF);
  harness.sessionSend.mockResolvedValue(true);
  harness.sessionAttachmentRead.mockResolvedValue({ mimeType: "image/png", data: "aGk=" });
  harness.sessionAttachmentDelete.mockResolvedValue(undefined);
  harness.sessionQueueAdd.mockResolvedValue(undefined);
  harness.sessionUploadBegin.mockResolvedValue(0);
  harness.sessionUploadStatus.mockResolvedValue(0);
  harness.sessionUploadChunk.mockImplementation(
    async (_id: string, _session: string, _upload: string, offset: number, data: string) =>
      offset + atob(data).length,
  );
  harness.sessionUploadFinish.mockResolvedValue(FILE_REF);
  harness.sessionUploadAbort.mockResolvedValue(undefined);
  vi.stubGlobal("IntersectionObserver", undefined);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("chat surface files", () => {
  it("uploads the picked file through the frames and sends its reference", async () => {
    await renderSurface();
    await pickFile();
    expect(harness.sessionUploadBegin).toHaveBeenCalledWith(
      SESSION,
      SESSION,
      expect.stringMatching(/^u/),
      "report.pdf",
      4,
    );
    expect(harness.sessionUploadFinish).toHaveBeenCalledTimes(1);
    expect(chipReady()).toBe(true);

    await typeText("summarise the attached file");
    await pressSend();

    expect(harness.sessionSend).toHaveBeenCalledTimes(1);
    const sendCall = harness.sessionSend.mock.calls[0]!;
    expect(sendCall[2]).toBe("summarise the attached file");
    expect(sendCall[5]).toEqual([FILE_REF]);
    expect(chipText()).toBe("");
  });

  it("keeps the chip when the send is refused", async () => {
    harness.sessionSend.mockRejectedValue(new Error("the session refused the prompt"));
    await renderSurface();
    await pickFile();
    await typeText("summarise the attached file");
    await pressSend();
    expect(chipReady()).toBe(true);
  });

  it("releases the stored bytes when a ready chip is removed", async () => {
    await renderSurface();
    await pickFile();
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label^="Remove attached file"]')!
        .click();
    });
    expect(harness.sessionAttachmentDelete).toHaveBeenCalledWith(FILE_REF);
    expect(container.querySelector('[data-testid="composer-file-chip"]')).toBeNull();
  });

  it("clears the chips only after the queued add is accepted", async () => {
    harness.sessionQueueAdd.mockRejectedValue({
      code: "invalid_request",
      message: "A queued message needs text or an attachment.",
    });
    await renderSurface({ activity: "working", queueSupported: true });
    await pickImage();
    await pickFile();
    await typeText("look at both");
    await pressSend();

    expect(harness.sessionQueueAdd).toHaveBeenCalledTimes(1);
    const queueCall = harness.sessionQueueAdd.mock.calls[0]!;
    expect(queueCall[4]).toEqual([IMAGE_REF, FILE_REF]);
    expect(chipReady()).toBe(true);

    harness.sessionQueueAdd.mockResolvedValue(undefined);
    await pressSend();
    expect(container.querySelector('[data-testid="composer-file-chip"]')).toBeNull();
  });
});
