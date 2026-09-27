// Source test, not a layout test. It reads the declarations that make the
// squeezed-tab defect impossible and fails when one of them is deleted; it cannot
// see a clipped pixel. happy-dom computes no layout, and a programmatic `.click()`
// bypasses hit-testing — which is how the old hover pills stayed green while
// owning none of their own pixels (D8, night field test of 18 September), until
// they covered a tab's label and an ordinary click archived it. Whether the
// close chip covers what it should and nothing else stays a live check.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./strip.css", import.meta.url), "utf8");
const tsx = readFileSync(new URL("./SessionStrip.tsx", import.meta.url), "utf8");

/** The body of the top-level rule whose selector is exactly `selector`. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`, "m"))?.[1];
  if (body === undefined) throw new Error(`no rule in strip.css for ${selector}`);
  return body;
}

describe("the session strip", () => {
  it("scrolls its tabs instead of squeezing them", () => {
    const scroller = ruleBody(".workspace-session-tabs-scroll");
    expect(scroller).toContain("overflow-x: auto;");
    // A lone `overflow-x: auto` computes the other axis to `auto`, and the row
    // then grows a vertical bar beside 28 px tabs.
    expect(scroller).toContain("overflow-y: hidden;");
  });

  it("shrinks chips toward the 96 px floor before the strip scrolls", () => {
    // The row shares a shortfall proportionally and stops at the floor;
    // past it the scrollport overflows and scrolls instead.
    const row = ruleBody(".workspace-session-row");
    expect(row).not.toContain("flex: none;");
    expect(row).toContain("min-width:");
    expect(row).toContain("96px");
    const chip = ruleBody(".workspace-session-tab");
    expect(chip).toContain("min-width: 96px;");
    expect(chip).toContain("max-width: 160px;");
  });

  it("hides the native scrollbar on the scrollport", () => {
    const scroller = ruleBody(".workspace-session-tabs-scroll");
    expect(scroller).toContain("scrollbar-width: none;");
    const webkit = css.match(
      /\.workspace-session-tabs-scroll::-webkit-scrollbar \{([\s\S]*?)\n\}/,
    )?.[1];
    expect(webkit).toContain("display: none;");
  });

  it("fades only the sides that still hide chips", () => {
    const side = (side: string): string => {
      const body = css.match(
        new RegExp(
          `\\.workspace-session-tabs-scroll\\[data-fade-${side}="true"\\] \\{([\\s\\S]*?)\\n\\}`,
        ),
      )?.[1];
      if (body === undefined) throw new Error(`no fade rule for ${side}`);
      return body;
    };
    for (const fadeSide of ["left", "right"]) {
      expect(side(fadeSide)).toContain("mask-image:");
      expect(side(fadeSide)).toContain("36px");
    }
    const both = css.match(
      /\.workspace-session-tabs-scroll\[data-fade-left="true"\]\[data-fade-right="true"\] \{([\s\S]*?)\n\}/,
    )?.[1];
    expect(both).toContain("mask-image:");
  });

  it("keeps the close chip a narrow trailing overlay that hides unclickable", () => {
    // Paseo's chip: ~48 px on the row's right edge — never a full-width hit
    // area over the label — and hidden means a pointer cannot reach it.
    const chip = ruleBody(".workspace-session-chip");
    expect(chip).toContain("width: 48px;");
    expect(chip).toContain("pointer-events: none;");
    expect(chip).toContain("visibility: hidden;");
    const shown = css.match(
      /\.workspace-session-row:hover \.workspace-session-chip,\n\.workspace-session-row:focus-within \.workspace-session-chip \{([\s\S]*?)\n\}/,
    )?.[1];
    expect(shown).toContain("pointer-events: auto;");
  });

  it("stays a single row", () => {
    // Wrapping would put a second 36 px row of tabs over the panel's content.
    expect(ruleBody(".workspace-session-tabs")).not.toContain("flex-wrap: wrap");
  });

  it("leaves the add button outside the box that scrolls", () => {
    // Rows render through StripChip now, so div-counting cannot see the
    // structure; the placement pin lives in the source order instead: the
    // scrollport's closing tag comes before the add wrapper opens, and the
    // DOM test in SessionStrip.test.tsx proves the same on elements.
    const scrollerAt = tsx.indexOf("workspace-session-tabs-scroll");
    expect(scrollerAt).toBeGreaterThan(-1);
    const addWrapAt = tsx.indexOf("workspace-session-add-wrap");
    expect(addWrapAt).toBeGreaterThan(scrollerAt);
    const between = tsx.slice(scrollerAt, addWrapAt);
    expect(between).toContain("</div>");
    expect(between).not.toContain('workspace-session-add"');
  });

  it("marks multi-selected tabs distinctly from the active tab", () => {
    // The active tab is a filled background; multi-select must read as a
    // separate thing, not as "this tab is the one playing".
    const multi = ruleBody(".workspace-session-tab-multiselected");
    const active = ruleBody(".workspace-session-tab-selected");
    expect(multi).toContain("outline:");
    expect(multi).not.toBe(active);
    expect(active).toContain("background: var(--selection);");
  });
});
