// A surface with its own light ground can sit inside one that stays dark in both
// themes (a copyable fence in an open plan tool row), and it would inherit the
// dark surface's ring and button pair: 1.55:1 on the light fill. Each such
// surface is registered here, and its rule must hand its own ground's pair back
// (the ring the accent, the button tokens unset) and keep its own text colour.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { contrastRatio } from "./contrast";
import { find, groundOf, hex, label, THEMES, varName } from "./sheetRules";

interface NestedLightSurface {
  file: string;
  selector: string;
  /** Whether it holds a control that rings or paints a button. */
  holdsControls: boolean;
  /** Where it can sit inside a dark surface. */
  inside: string;
  text: string;
}

const NESTED_LIGHT_SURFACES: readonly NestedLightSurface[] = [
  {
    file: "src/components/codeBlocks.css",
    selector: ".copyblock",
    holdsControls: true,
    inside: "a markdown fence in an open plan tool row",
    text: "--ink",
  },
  {
    file: "src/components/markdown.css",
    selector: ".plan-markdown-table th",
    holdsControls: false,
    inside: "a markdown table in an open plan tool row",
    text: "--ink",
  },
];

describe("light surfaces nested in the surfaces that stay dark in both themes", () => {
  for (const surface of NESTED_LIGHT_SURFACES) {
    const rule = find(surface.file, surface.selector);

    it(`${surface.selector} (${surface.inside}) keeps its own text on its own ground`, () => {
      expect(rule, `${surface.file}: ${surface.selector} is missing`).toBeDefined();
      expect(varName(rule!.declarations.get("color"))).toBe(surface.text);
      const ground = groundOf(rule!);
      expect(ground, `${label(rule!)} paints no single ground token`).not.toBeNull();
      for (const theme of THEMES) {
        const ratio = contrastRatio(hex(surface.text, theme), hex(ground!, theme));
        expect(ratio, `${surface.text} on ${ground} (${theme})`).toBeGreaterThanOrEqual(4.5);
      }
    });

    if (surface.holdsControls) {
      it(`${surface.selector} hands the ring and the button pair back to its own ground`, () => {
        expect(rule!.declarations.get("--ring")).toBe("var(--accent)");
        for (const token of ["--button-fill", "--button-fill-hover", "--button-text"]) {
          expect(rule!.declarations.get(token), `${label(rule!)}: ${token}`).toBe("initial");
        }
      });
    }
  }
});
