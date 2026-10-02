import type { PromptAttachment } from "../../types/ipc";
import type { AttachmentReference } from "../../lib/tauri";

/**
 * Deposit the composer's images and send their references.
 *
 * One deposit per image, in composer order, then one send naming the answered
 * references: the echo carries the names, and replay resolves the stored
 * bytes from them. A refused deposit rejects before anything is sent, so a
 * prompt never goes out missing an image it claimed.
 */
export async function sendChatImagesByReference<T>(args: {
  images: readonly PromptAttachment[];
  deposit: (attachment: PromptAttachment) => Promise<AttachmentReference>;
  send: (references: readonly AttachmentReference[]) => Promise<T>;
}): Promise<T> {
  const references: AttachmentReference[] = [];
  for (const image of args.images) references.push(await args.deposit(image));
  return args.send(references);
}
