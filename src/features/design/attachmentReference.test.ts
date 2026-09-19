// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { ATTACHMENT_URL_SCHEME, parseAttachmentReference } from "./attachmentReference";

const DIGEST = "a".repeat(64);

function part(url: string, storedBytes: unknown): Parameters<typeof parseAttachmentReference>[0] {
  return { url, metadata: { storedBytes: storedBytes as number } };
}

describe("parseAttachmentReference", () => {
  it("turns a well-formed part into the reference the read door takes", () => {
    expect(
      parseAttachmentReference(part(`devboule-attachment:s.creator.1/${DIGEST}`, 512)),
    ).toEqual({ sessionId: "s.creator.1", digest: DIGEST, storedBytes: 512 });
  });

  it("refuses a url of any other shape rather than guessing", () => {
    const cases: Array<[string, unknown]> = [
      ["https://example.com/a.png", 512],
      ["devboule-attachment", 512],
      ["devboule-attachment:", 512],
      [`devboule-attachment:/${DIGEST}`, 512],
      ["devboule-attachment:s.creator.1/", 512],
      [`devboule-attachment:s.creator.1/${DIGEST}/extra`, 512],
      ["Devboule-Attachment:s.creator.1/" + DIGEST, 512],
      [`devboule-attachment :s.creator.1/${DIGEST}`, 512],
    ];
    for (const [url, storedBytes] of cases) {
      expect(parseAttachmentReference(part(url, storedBytes)), url).toBeNull();
    }
  });

  it("refuses every digest spelling the daemon would refuse", () => {
    const prefix = "devboule-attachment:s.creator.1/";
    const cases = [
      "A".repeat(64),
      "a".repeat(63),
      "a".repeat(65),
      "g".repeat(64),
      "",
      "a".repeat(63) + " ",
    ];
    for (const digest of cases) {
      expect(parseAttachmentReference(part(prefix + digest, 512)), digest).toBeNull();
    }
  });

  it("refuses a size that is absent, fractional or negative", () => {
    const url = `devboule-attachment:s.creator.1/${DIGEST}`;
    for (const storedBytes of [undefined, null, "512", 512.5, -1, Number.NaN]) {
      expect(parseAttachmentReference(part(url, storedBytes)), String(storedBytes)).toBeNull();
    }
    expect(parseAttachmentReference(part(url, 0))).toEqual({
      sessionId: "s.creator.1",
      digest: DIGEST,
      storedBytes: 0,
    });
  });

  it("walks the daemon's url literal against the scheme this parser expects", () => {
    // The producer is `deposit_child_message` in the daemon's `session_messaging.rs`;
    // this test reads the Rust source so a changed scheme or separator
    // cannot land there and stay invisible here — nothing else makes the
    // two agree and no compiler sees the seam.
    const source = readFileSync(
      join("crates", "devboule-daemon", "src", "session_messaging.rs"),
      "utf8",
    );
    const literal = source.match(/format!\(\s*"(devboule-attachment:)\{\}(\/)\{\}"/);
    if (literal === null) {
      throw new Error("attachment url literal not found in session_messaging.rs");
    }
    expect(literal[1]).toBe(ATTACHMENT_URL_SCHEME);
    expect(literal[2]).toBe("/");
  });
});
