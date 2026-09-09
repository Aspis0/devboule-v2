import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
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

  it("guards the install error band against the arc stroke", () => {
    const globalCss = readFileSync(new URL("../styles/global.css", import.meta.url), "utf8");
    const errorRule = globalCss.match(/\.crescent-install-error\s*\{([\s\S]*?)\n\}/)?.[1];
    const buttonRule = globalCss.match(/\.crescent-install-error button\s*\{([\s\S]*?)\n\}/)?.[1];
    const arcPathRule = globalCss.match(/\.crescent-arc path\s*\{([\s\S]*?)\n\}/)?.[1];

    function cssNumber(rule: string | undefined, property: string): number {
      if (rule === undefined) throw new Error(`Missing CSS rule for ${property}`);
      const match = rule.match(new RegExp(`${property}:\\s*([\\d.]+)`));
      if (match === null) throw new Error(`Missing CSS property ${property}`);
      return Number(match[1]);
    }

    expect(errorRule).toBeDefined();
    expect(errorRule).toContain("top: 0;");
    expect(buttonRule).toBeDefined();
    expect(buttonRule).toContain("border: 0;");
    expect(arcPathRule).toBeDefined();

    const top = cssNumber(errorRule, "top");
    const fontSize = cssNumber(errorRule, "font-size");
    const lineHeight = cssNumber(errorRule, "line-height");
    const strokeWidth = cssNumber(arcPathRule, "stroke-width");
    expect(top + fontSize * lineHeight).toBeLessThanOrEqual(CRESCENT_ARC_Y - strokeWidth / 2);
  });
});
