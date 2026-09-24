// Source test, not a layout test. It reads the declarations that make the
// squeezed-tab defect impossible and fails when one of them is deleted; it cannot
// see a clipped pixel. happy-dom computes no layout, and a programmatic `.click()`
// bypasses hit-testing — which is how the old hover pills stayed green while
// owning none of their own pixels (D8, night field test of 18 September), until
// they covered a tab's label and an ordinary click archived it. Whether the
// close chip covers what it should and nothing else stays a live check.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./Workspace.css", import.meta.url), "utf8");
const tsx = readFileSync(new URL("./Workspace.tsx", import.meta.url), "utf8");

/** The body of the top-level rule whose selector is exactly `selector`. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`, "m"))?.[1];
  if (body === undefined) throw new Error(`no rule in Workspace.css for ${selector}`);
  return body;
}

describe("the session strip", () => {
  it("scrolls its tabs instead of squeezing them", () => {
    const scroller = ruleBody(".workspace-session-tabs-scroll");
    expect(scroller).toContain("overflow-x: auto;");
    // A lone `overflow-x: auto` computes the other axis to `auto`, and the row
    // then grows a vertical bar beside 29 px tabs.
    expect(scroller).toContain("overflow-y: hidden;");
  });

  it("forbids the row from shrinking a tab below its content", () => {
    expect(ruleBody(".workspace-session-row")).toContain("flex: none;");
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
    // Wrapping would put a second 44 px row of tabs over the panel's content.
    expect(ruleBody(".workspace-session-tabs")).not.toContain("flex-wrap: wrap");
  });

  it("leaves the add button outside the box that scrolls", () => {
    const scrollerAt = tsx.indexOf("workspace-session-tabs-scroll");
    expect(scrollerAt).toBeGreaterThan(-1);
    const openEnd = tsx.indexOf(">", scrollerAt) + 1;
    // Where the add button's wrapper opens, every box opened before it —
    // the scroller and each tab row inside it — is closed again: the
    // wrapper is the scroller's sibling, not a passenger. (The one div
    // still open at that point in the source is the wrapper's own.)
    const addWrapAt = tsx.indexOf("workspace-session-add-wrap");
    expect(addWrapAt).toBeGreaterThan(openEnd);
    const between = tsx.slice(openEnd, addWrapAt);
    const opened = (between.match(/<div/g) ?? []).length;
    const closed = (between.match(/<\/div>/g) ?? []).length;
    expect(closed).toBe(opened);
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
