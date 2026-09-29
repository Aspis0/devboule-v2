import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { assembleCssProof } from "../features/workspace/cssProof";
import {
  CRESCENT_ARC_END_X,
  CRESCENT_ARC_RADIUS,
  CRESCENT_ARC_START_X,
  CRESCENT_ARC_Y,
  CRESCENT_LABEL_MAX_WIDTH,
  CRESCENT_PAGE_ARROW_NEXT_RIGHT,
  CRESCENT_PAGE_ARROW_PREV_LEFT,
  CRESCENT_PAGE_ARROW_WIDTH,
  CRESCENT_SHELL_WIDTH,
  layoutCrescent,
} from "./crescentLayout";

const proof = assembleCssProof([
  readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8"),
  readFileSync(new URL("../styles/global.css", import.meta.url), "utf8"),
]);

function cssNumber(ruleBody: string | undefined, property: string): number {
  if (ruleBody === undefined) throw new Error(`Missing CSS rule for ${property}`);
  const match = ruleBody.match(new RegExp(`${property}:\\s*([\\d.]+)`));
  if (match === null) throw new Error(`Missing CSS property ${property}`);
  return Number(match[1]);
}

// A rule without its own line-height inherits the nearest ancestor's, so
// read the declaration down the chain and inherit only past a silent rule.
function cssLineHeight(ruleBody: string, inherited: number): number {
  try {
    return cssNumber(ruleBody, "line-height");
  } catch {
    return inherited;
  }
}

// The circle inherits through the chain as rendered (Shell.tsx): its own
// rule, then button.nav-point, .crescent-nav, .crescent-shell, then the page.
// Nearest set leading wins: the value that actually applies.
function cssCircleLineHeight(
  baseBody: string,
  ancestors: readonly string[],
  bodyHeight: number,
): number {
  let inherited = bodyHeight;
  for (let index = ancestors.length - 1; index >= 0; index -= 1) {
    inherited = cssLineHeight(ancestors[index]!, inherited);
  }
  return cssLineHeight(baseBody, inherited);
}

describe("layoutCrescent", () => {
  it("keeps every visible point between the crescent arc ends in order", () => {
    const layout = layoutCrescent(["a", "b", "c", "d", "e", "f"], 6, 0);
    expect(layout.points).toHaveLength(6);

    const xPositions = layout.points.map((point) => point.x);
    expect(xPositions.every((x) => x >= CRESCENT_ARC_START_X && x <= CRESCENT_ARC_END_X)).toBe(
      true,
    );
    expect(xPositions).toEqual([...xPositions].sort((left, right) => left - right));

    const centerX = (CRESCENT_ARC_START_X + CRESCENT_ARC_END_X) / 2;
    const halfChord = (CRESCENT_ARC_END_X - CRESCENT_ARC_START_X) / 2;
    const centerY = CRESCENT_ARC_Y - Math.sqrt(CRESCENT_ARC_RADIUS ** 2 - halfChord ** 2);
    for (const point of layout.points) {
      expect(
        Math.abs(Math.hypot(point.x - centerX, point.y - centerY) - CRESCENT_ARC_RADIUS),
      ).toBeLessThanOrEqual(0.05);
      expect(point.y).toBeGreaterThanOrEqual(CRESCENT_ARC_Y);
    }
  });

  it("does not offer paging when every key fits", () => {
    const layout = layoutCrescent(["a", "b", "c"], 6, 0);

    expect(layout.canPrev).toBe(false);
    expect(layout.canNext).toBe(false);
  });

  it("pages a window when more keys exist than the visible capacity", () => {
    const keys = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m"];
    const first = layoutCrescent(keys, 6, 0);
    const offsetOne = layoutCrescent(keys, 6, 1);
    const offsetSix = layoutCrescent(keys, 6, 6);

    expect(first.canPrev).toBe(false);
    expect(first.canNext).toBe(true);
    expect(offsetOne.canPrev).toBe(true);
    expect(offsetOne.canNext).toBe(true);
    expect(offsetOne.visibleKeys).toEqual(keys.slice(1, 7));
    expect(offsetSix.visibleKeys).toEqual(keys.slice(6, 12));
    expect(offsetOne.visibleKeys).not.toEqual(offsetSix.visibleKeys);
  });

  it("keeps the six labels clear of the paging arrows", () => {
    const globalCss = readFileSync(new URL("../styles/global.css", import.meta.url), "utf8");
    const shellRule = globalCss.match(/\.crescent-shell\s*\{([\s\S]*?)\n\}/)?.[1];
    const previousArrowRule = globalCss.match(/\.crescent-page-arrow-prev\s*\{([\s\S]*?)\n\}/)?.[1];
    const nextArrowRule = globalCss.match(/\.crescent-page-arrow-next\s*\{([\s\S]*?)\n\}/)?.[1];

    expect(shellRule).toBeDefined();
    expect(shellRule).toContain("width: 880px;");
    expect(previousArrowRule).toBeDefined();
    expect(previousArrowRule).toContain("left: 201px;");
    expect(nextArrowRule).toBeDefined();
    expect(nextArrowRule).toContain("right: 141px;");

    const layout = layoutCrescent(["a", "b", "c", "d", "e", "f"], 6, 0);
    const firstPoint = layout.points[0];
    const lastPoint = layout.points.at(-1);
    if (firstPoint === undefined || lastPoint === undefined) {
      throw new Error("six-point crescent did not render");
    }

    expect(lastPoint.x + CRESCENT_LABEL_MAX_WIDTH / 2).toBeLessThanOrEqual(
      CRESCENT_SHELL_WIDTH - CRESCENT_PAGE_ARROW_NEXT_RIGHT - CRESCENT_PAGE_ARROW_WIDTH,
    );
    expect(firstPoint.x - CRESCENT_LABEL_MAX_WIDTH / 2).toBeGreaterThanOrEqual(
      CRESCENT_PAGE_ARROW_PREV_LEFT + CRESCENT_PAGE_ARROW_WIDTH,
    );
  });

  it("binds the .nav-point-label max-width in CSS to CRESCENT_LABEL_MAX_WIDTH", () => {
    const globalCss = readFileSync(new URL("../styles/global.css", import.meta.url), "utf8");
    const labelRule = globalCss.match(/\.nav-point-label\s*\{([\s\S]*?)\n\}/)?.[1];
    expect(labelRule).toBeDefined();
    const maxWidth = labelRule?.match(/max-width:\s*([\d.]+)px/);
    expect(maxWidth).not.toBeNull();
    expect(Number(maxWidth?.[1])).toBe(CRESCENT_LABEL_MAX_WIDTH);
  });

  it("guards the install error band against the arc stroke", () => {
    const errorRule = proof.rulesFor(".crescent-install-error");
    const buttonRule = proof.rulesFor(".crescent-install-error button");
    const arcPathRule = proof.rulesFor(".crescent-arc path");

    expect(errorRule).toContain("top: 0;");
    expect(buttonRule).toContain("border: 0;");
    expect(arcPathRule).not.toBe("");

    const top = cssNumber(errorRule, "top");
    const fontSize = cssNumber(errorRule, "font-size");
    const lineHeight = cssNumber(errorRule, "line-height");
    const strokeWidth = cssNumber(arcPathRule, "stroke-width");
    expect(top + fontSize * lineHeight).toBeLessThanOrEqual(CRESCENT_ARC_Y - strokeWidth / 2);
  });

  it("paints the + glyph on the other nav glyphs' optical centre", () => {
    // The + rides --type-small, not a bare literal beside the tokenised base.
    const rawGlobalCss = readFileSync(new URL("../styles/global.css", import.meta.url), "utf8");
    const addOverride = rawGlobalCss.match(/\.nav-point-add \.nav-point-circle\s*\{([^}]*)\}/)?.[1];
    expect(addOverride).toContain("font-size: var(--type-small);");

    // Half the line-box difference is the glyph's vertical drift off the
    // row's centre; the budget is 0.5px.
    const base = proof.rulesFor(".nav-point-circle");
    const add = proof.rulesFor(".nav-point-add .nav-point-circle");
    const parent = proof.rulesFor(".nav-point");
    const nav = proof.rulesFor(".crescent-nav");
    const shell = proof.rulesFor(".crescent-shell");
    const bodyLineHeight = cssNumber(proof.rulesFor("body"), "line-height");
    const baseLineBox =
      cssNumber(base, "font-size") *
      cssCircleLineHeight(base, [parent, nav, shell], bodyLineHeight);
    const addLineBox = cssNumber(add, "font-size") * cssNumber(add, "line-height");
    expect(Math.abs(baseLineBox - addLineBox) / 2).toBeLessThanOrEqual(0.5);
  });
});
