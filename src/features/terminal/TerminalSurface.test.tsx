import { describe, expect, it } from "vitest";
import { bannerText } from "./TerminalSurface";

describe("terminal integrity banner copy", () => {
  it("renders the zero-byte recovered warning verbatim", () => {
    expect(
      bannerText({
        kind: "recovered",
        integrity: {
          kind: "unverifiable",
          droppedFrames: 0,
          droppedBytes: 0,
          trimmedBytes: 0,
        },
      }),
    ).toBe(
      "The previous terminal process is gone. The end of the saved transcript could not be verified.",
    );
  });

  it("renders the measured recovered warning verbatim", () => {
    expect(
      bannerText({
        kind: "recovered",
        integrity: {
          kind: "unverifiable",
          droppedFrames: 2,
          droppedBytes: 12 * 1024,
          trimmedBytes: 0,
        },
      }),
    ).toBe(
      "The previous terminal process is gone. At least 12 KB of output was not saved, and the end of the transcript could not be verified either.",
    );
  });

  it("renders the measured exited warning verbatim", () => {
    expect(
      bannerText({
        kind: "exited",
        code: 7,
        lost: { frames: 2, bytes: 12 * 1024 },
        trimmedBytes: 0,
      }),
    ).toBe("The terminal process exited with code 7. At least 12 KB of output was not saved.");
  });

  it("renders the unknown-amount exited warning verbatim", () => {
    expect(
      bannerText({
        kind: "exited",
        code: 7,
        lost: { frames: 2, bytes: 0 },
        trimmedBytes: 0,
      }),
    ).toBe("The terminal process exited with code 7. Some output was not saved.");
  });

  it("renders the live journal degradation warning verbatim", () => {
    expect(bannerText({ kind: "journal_degraded", lost: { frames: 2, bytes: 12 * 1024 } })).toBe(
      "Scrollback history is incomplete: at least 12 KB of output could not be saved.",
    );
  });

  it("keeps the exited copy shape when no exit code was observed", () => {
    expect(
      bannerText({
        kind: "exited",
        code: null,
        lost: { frames: 2, bytes: 12 * 1024 },
        trimmedBytes: 0,
      }),
    ).toBe("The terminal process exited. At least 12 KB of output was not saved.");
  });

  it("renders the trimmed-only warning verbatim", () => {
    expect(bannerText({ kind: "exited", code: 0, lost: null, trimmedBytes: 12 * 1024 })).toBe(
      "The oldest 12 KB of this transcript was removed by the history limit.",
    );
  });

  it("renders the trimmed-and-lost warning verbatim", () => {
    expect(
      bannerText({
        kind: "exited",
        code: 0,
        lost: { frames: 2, bytes: 8 * 1024 },
        trimmedBytes: 12 * 1024,
      }),
    ).toBe(
      "The oldest 12 KB was removed by the history limit, and at least 8.2 KB of output was not saved.",
    );
  });
});
