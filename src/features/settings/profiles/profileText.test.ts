// @vitest-environment happy-dom

// The profile row's display strings: the meta line reads provider · model ·
// effort, with the serving provider between the first two when the profile
// stores one — and the glyph tile's content.
import { describe, expect, it } from "vitest";
import { profileMetaText, profileTileText } from "./profileText";

describe("profileMetaText", () => {
  it("names provider, model and effort", () => {
    expect(
      profileMetaText({
        provider: "claude",
        model: "opus-4-6",
        thinkingOptionId: "high",
      }),
    ).toBe("claude · opus-4-6 · high");
  });

  it("names the serving provider between provider and model", () => {
    expect(
      profileMetaText({
        provider: "pi",
        model: "mimo-v2-6-flash",
        modelProvider: "opencode-go",
        thinkingOptionId: "High",
      }),
    ).toBe("pi · opencode-go · mimo-v2-6-flash · High");
  });

  it("says nothing about effort when the profile stores none", () => {
    expect(
      profileMetaText({
        provider: "pi",
        model: "mimo-2-6",
        thinkingOptionId: null,
      }),
    ).toBe("pi · mimo-2-6");
  });

  it("reads a bare stored id exactly as before", () => {
    expect(
      profileMetaText({
        provider: "pi",
        model: "mimo-2-6",
        modelProvider: null,
        thinkingOptionId: "  ",
      }),
    ).toBe("pi · mimo-2-6");
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
