// A `FinishArtifactPart` url is a reference, never a path: this turns one
// into the `AttachmentReference` the read door takes, or refuses it.

import type { AttachmentReference } from "../../lib/tauri";
import type { FinishArtifactPart } from "../../types/ipc";

/** The scheme the daemon writes before every attachment url. */
export const ATTACHMENT_URL_SCHEME = "devboule-attachment:";

/** A digest as the store writes it: SHA-256, lowercase hex. */
const DIGEST_PATTERN = /^[0-9a-f]{64}$/;

/**
 * The reference a part names, or null when it names nothing readable.
 * Refuses rather than guesses: a bad scheme, a missing half, a digest of
 * any other spelling, or a size that is absent, fractional or negative all
 * answer null, because the daemon refuses every one of those and a round
 * trip would only learn that again.
 */
export function parseAttachmentReference(
  part: Pick<FinishArtifactPart, "url" | "metadata">,
): AttachmentReference | null {
  if (!part.url.startsWith(ATTACHMENT_URL_SCHEME)) return null;
  const rest = part.url.slice(ATTACHMENT_URL_SCHEME.length);
  const slash = rest.indexOf("/");
  if (slash < 0) return null;
  const sessionId = rest.slice(0, slash);
  const digest = rest.slice(slash + 1);
  if (sessionId.length === 0 || digest.length === 0) return null;
  if (digest.includes("/")) return null;
  if (!DIGEST_PATTERN.test(digest)) return null;
  const storedBytes = part.metadata?.storedBytes;
  if (typeof storedBytes !== "number" || !Number.isInteger(storedBytes) || storedBytes < 0) {
    return null;
  }
  return { sessionId, digest, storedBytes };
}
