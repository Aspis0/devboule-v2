// @vitest-environment happy-dom

// The profile row's display strings: the meta line the spec fixes as
// `provider · model · mode · thinking`, and the glyph tile's content.
import { describe, expect, it } from "vitest";
import { profileMetaText, profileTileText } from "./profileText";

describe("profileMetaText", () => {
  it("names provider, model, mode and thinking", () => {
    expect(
      profileMetaText({
        provider: "claude",
        model: "opus-4-6",
        modeId: "agent",
        thinkingOptionId: "high",
      }),
    ).toBe("claude · opus-4-6 · agent · high thinking");
  });

  it("says nothing about thinking when the profile stores none", () => {
    expect(
      profileMetaText({
        provider: "pi",
        model: "mimo-2-6",
        modeId: "agent",
        thinkingOptionId: null,
      }),
    ).toBe("pi · mimo-2-6 · agent");
  });

  it("treats blank thinking as unset", () => {
    expect(
      profileMetaText({
        provider: "pi",
        model: "mimo-2-6",
        modeId: "agent",
        thinkingOptionId: "  ",
      }),
    ).toBe("pi · mimo-2-6 · agent");
  });
});

describe("profileTileText", () => {
  it("shows the stored icon when there is one", () => {
    expect(profileTileText({ name: "Coder", icon: "✦" })).toBe("✦");
  });

  it("falls back to the name's first letter", () => {
    expect(profileTileText({ name: "Coder", icon: null })).toBe("C");
    expect(profileTileText({ name: "coder", icon: "" })).toBe("C");
  });
});
