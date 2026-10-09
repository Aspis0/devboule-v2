// A button inside a surface that stays dark in both themes (code blocks, error
// boundaries, permission cards) must not fall back to the browser's button fill,
// which is light in the light theme and leaves the inherited near-white text unreadable.
// Such a surface sets the ring (ringConsumers.walk.test.ts) and the button pair
// together; the global button rule reads them. This walk proves:
//   1. every rule that sets the on-dark ring also sets the dark button fill,
//      its hover fill and its text;
//   2. the text reaches 4.5:1 on the fill and on the hover fill, and the ring
//      reaches 3:1 on both, in both themes;
//   3. the global button rules are the ones that read those tokens.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { contrastRatio } from "./contrast";
import { find, hex, label, RULES, THEMES, varName } from "./sheetRules";

const ON_DARK_RING = "var(--accent-on-code)";
const TEXT_FLOOR = 4.5;
const RING_FLOOR = 3;

const SURFACE_BUTTON_TOKENS = {
  "--button-fill": "var(--code-control)",
  "--button-fill-hover": "var(--code-control-hover)",
  "--button-text": "var(--code-text)",
} as const;

const darkSurfaces = RULES.filter((rule) => rule.declarations.get("--ring") === ON_DARK_RING);

describe("buttons inside the surfaces that stay dark in both themes", () => {
  it("found the dark surfaces", () => {
    expect(darkSurfaces.length).toBeGreaterThanOrEqual(3);
  });

  it("every dark surface sets the button fill, hover fill and text", () => {
    const missing = darkSurfaces.flatMap((rule) =>
      Object.entries(SURFACE_BUTTON_TOKENS)
        .filter(([name, value]) => rule.declarations.get(name) !== value)
        .map(([name, value]) => `${label(rule)}: ${name} is not ${value}`),
    );
    expect(missing).toEqual([]);
  });

  it("the text reads on the fill and on the hover fill, and the ring reads on both", () => {
    const failures: string[] = [];
    for (const rule of darkSurfaces) {
      const text = varName(rule.declarations.get("--button-text"));
      const ring = varName(rule.declarations.get("--ring"));
      for (const fillName of ["--button-fill", "--button-fill-hover"]) {
        const fill = varName(rule.declarations.get(fillName));
        if (text === null || ring === null || fill === null) {
          failures.push(`${label(rule)}: ${fillName}, --button-text and --ring need var() values`);
          continue;
        }
        for (const theme of THEMES) {
          const textRatio = contrastRatio(hex(text, theme), hex(fill, theme));
          if (textRatio < TEXT_FLOOR) {
            failures.push(
              `${label(rule)} (${theme}): ${text} on ${fill} is ${textRatio.toFixed(2)}`,
            );
          }
          const ringRatio = contrastRatio(hex(ring, theme), hex(fill, theme));
          if (ringRatio < RING_FLOOR) {
            failures.push(
              `${label(rule)} (${theme}): ${ring} on ${fill} is ${ringRatio.toFixed(2)}`,
            );
          }
        }
      }
    }
    expect(failures).toEqual([]);
  });

  it("the global button rules read the surface's pair and change nothing elsewhere", () => {
    const rest = find("src/styles/global.css", ":where(button)");
    const hover = find("src/styles/global.css", ":where(button:hover)");
    expect(rest, "the :where(button) rule is missing").toBeDefined();
    expect(hover, "the :where(button:hover) rule is missing").toBeDefined();
    // Defaults equal the browser's own button paint, so a button outside a dark
    // surface is untouched; :where() keeps their specificity at zero, so any
    // class-styled button still wins.
    expect(rest!.declarations.get("background")).toBe("var(--button-fill, ButtonFace)");
    expect(rest!.declarations.get("color")).toBe("var(--button-text, currentColor)");
    expect(hover!.declarations.get("background")).toBe(
      "var(--button-fill-hover, var(--button-fill, ButtonFace))",
    );
  });
});
