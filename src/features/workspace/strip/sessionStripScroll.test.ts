// Source test, not a layout test. It reads the declarations that keep the
// strip's overflow contract — shrink toward the 96 px floor, then scroll —
// and fails when one of them is deleted; it cannot see a clipped pixel.
// happy-dom computes no layout, and a programmatic `.click()`
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

/** The exact declared value of one property in a rule body. */
function declaredValue(body: string, property: string): string | undefined {
  return body.match(new RegExp(`^\\s*${property}\\s*:\\s*([^;]+);`, "m"))?.[1]?.trim();
}

describe("the session strip", () => {
  it("clips tabs that overflow instead of wrapping them", () => {
    const scroller = ruleBody(".workspace-session-tabs-scroll");
    expect(scroller).toContain("overflow-x: auto;");
    // A lone `overflow-x: auto` computes the other axis to `auto`, and the row
    // then grows a vertical bar beside 28 px tabs.
    expect(scroller).toContain("overflow-y: hidden;");
  });

  it("shrinks chips toward the 96 px floor before the strip scrolls", () => {
    // The row shares a shortfall proportionally and stops at the floor;
    // past it the scrollport overflows and scrolls instead. The floor is
    // asserted as the exact declared value: any `min-content` term in it
    // raises the floor to the label's own width and the strip never
    // shrinks. happy-dom computes no layout, so the real shrink is a live
    // check; this pins the declaration the live behaviour depends on.
    const row = ruleBody(".workspace-session-row");
    expect(row).not.toContain("flex: none;");
    expect(declaredValue(row, "min-width")).toBe("96px");
    // A row carrying the take-back never shrinks under its action.
    const takeBackRow = css.match(
      /\.workspace-session-row:has\(\.workspace-tab-takeback\) \{([\s\S]*?)\n\}/,
    )?.[1];
    expect(takeBackRow).toBeDefined();
    expect(declaredValue(takeBackRow ?? "", "min-width")).toBe("fit-content");
    const chip = ruleBody(".workspace-session-tab");
    expect(declaredValue(chip, "min-width")).toBe("96px");
    expect(declaredValue(chip, "max-width")).toBe("160px");
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
      expect(side(fadeSide)).toContain("-webkit-mask-image:");
      expect(side(fadeSide)).toContain("mask-image:");
      expect(side(fadeSide)).toContain("36px");
    }
    const both = css.match(
      /\.workspace-session-tabs-scroll\[data-fade-left="true"\]\[data-fade-right="true"\] \{([\s\S]*?)\n\}/,
    )?.[1];
    expect(both).toContain("-webkit-mask-image:");
    expect(both).toContain("mask-image:");
  });

  it("paints every dot tone from its own rule", () => {
    // A missing or misspelled tone rule paints a colourless 6 px dot — a
    // state rendering as nothing — so each tone pins its declaration here,
    // beside the component test that pins the class name.
    const tones: Array<[string, string]> = [
      [".strip-dot-live", "background: var(--green);"],
      [".strip-dot-attention", "background: var(--tone-attention);"],
      [".strip-dot-unattended", "background: var(--tone-unattended);"],
      [".strip-dot-recovered", "outline: 1.5px solid var(--tone-recovered);"],
      [".strip-dot-idle", "background: var(--border-strong);"],
      [".strip-dot-ended,\n.strip-dot-unknown", "background: var(--terracotta);"],
    ];
    for (const [selector, declaration] of tones) {
      expect(ruleBody(selector)).toContain(declaration);
    }
  });

  it("resolves selected+multiselected+hover to the selection fill on chip and scrim", () => {
    // All three hover-capable chip rules share specificity (0,2,0), and
    // both scrim rules share (0,3,1): ties break by source order. The
    // multi hover must sit before the selected hover so a both-classes
    // chip agrees with its scrim (selection), while a multi-only chip
    // still takes the hover fill from the earlier plain hover rule.
    const multiHoverAt = css.indexOf(".workspace-session-tab-multiselected:hover");
    const selectedHoverAt = css.indexOf(".workspace-session-tab-selected:hover");
    expect(multiHoverAt).toBeGreaterThan(-1);
    expect(selectedHoverAt).toBeGreaterThan(-1);
    expect(multiHoverAt).toBeLessThan(selectedHoverAt);
    const hoverScrimAt = css.indexOf(
      ".workspace-session-row:hover .workspace-session-chip::before",
    );
    const selectedScrimAt = css.indexOf(
      ".workspace-session-row:has(.workspace-session-tab-selected)",
    );
    expect(selectedScrimAt).toBeGreaterThan(hoverScrimAt);
  });

  it("keeps the hover look on multi-selected chips, chip and scrim alike", () => {
    // A hovered multi-selected chip keeps the hover fill: the multi rule
    // (0,1,0) already loses to :hover (0,2,0) on the chip, so the scrim
    // needs no multi rule of its own beating the hover scrim.
    const hovered = css.match(
      /\.workspace-session-tab-multiselected:hover,[\s\S]*?\{([\s\S]*?)\n\}/,
    )?.[1];
    expect(hovered).toContain("background: var(--fill-chip-hover);");
    expect(css).not.toContain(":has(.workspace-session-tab-multiselected)");
  });

  it("fades the label's tail under the close overlay on hover", () => {
    // The hover treatment is the right mask alone: adding padding here
    // would change the chip's flex base size and reflow the strip.
    // happy-dom has no layout; this pins the declarations, and the live
    // window judges the pixels.
    const hovered = css.match(
      /\.workspace-session-row:hover \.workspace-tab-label,[\s\S]*?\{([\s\S]*?)\n\}/,
    )?.[1];
    expect(hovered).not.toContain("padding");
    expect(hovered).toContain("mask-image:");
    expect(hovered).toContain("-webkit-mask-image:");
  });

  it("fades the close scrim instead of painting an opaque block", () => {
    // The overlay's ground is a gradient to transparent, per chip state,
    // so the fading label shows through it toward the ×.
    const hoverScrim = css.match(
      /\.workspace-session-row:hover \.workspace-session-chip::before,[\s\S]*?\{([\s\S]*?)\n\}/,
    )?.[1];
    expect(hoverScrim).toContain("linear-gradient");
    expect(hoverScrim).toContain("transparent");
    const selectedScrim = ruleBody(
      ".workspace-session-row:has(.workspace-session-tab-selected) .workspace-session-chip::before",
    );
    expect(selectedScrim).toContain("linear-gradient");
  });

  it("sets the session count in sans metadata type, never mono", () => {
    // Mono never appears in UI metadata: the count is 12 px sans --muted.
    const rate = ruleBody(".workspace-rate");
    expect(rate).not.toContain("monospace");
    expect(rate).not.toContain("Mono");
    expect(rate).toContain("12px");
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
