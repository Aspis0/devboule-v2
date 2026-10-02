// The chat send's deposit step: composer images are deposited once each and
// the send names the answered references, never the bytes.
import { describe, expect, it, vi } from "vitest";
import type { PromptAttachment } from "../../types/ipc";
import type { AttachmentReference } from "../../lib/tauri";
import { sendChatImagesByReference } from "./chatImageTransport";

const IMAGE: PromptAttachment = { name: "photo.png", mimeType: "image/png", data: "aGk=" };
const REF: AttachmentReference = {
  sessionId: "s.owner.chat1",
  digest: "d".repeat(64),
  storedBytes: 3,
};

describe("sendChatImagesByReference", () => {
  it("deposits each image once and sends the answered references", async () => {
    const deposit = vi.fn(async () => REF);
    const send = vi.fn(async () => true);
    const ok = await sendChatImagesByReference({ images: [IMAGE, IMAGE], deposit, send });
    expect(ok).toBe(true);
    expect(deposit).toHaveBeenCalledTimes(2);
    expect(deposit).toHaveBeenNthCalledWith(1, IMAGE);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith([REF, REF]);
  });

  it("sends with no references when the composer holds no images", async () => {
    const deposit = vi.fn(async () => REF);
    const send = vi.fn(async () => true);
    const ok = await sendChatImagesByReference({ images: [], deposit, send });
    expect(ok).toBe(true);
    expect(deposit).not.toHaveBeenCalled();
    expect(send).toHaveBeenCalledWith([]);
  });

  it("never sends when a deposit is refused", async () => {
    const deposit = vi.fn(async () => {
      throw new Error("Send /goal without attachments.");
    });
    const send = vi.fn(async () => true);
    await expect(sendChatImagesByReference({ images: [IMAGE], deposit, send })).rejects.toThrow();
    expect(send).not.toHaveBeenCalled();
  });
});
