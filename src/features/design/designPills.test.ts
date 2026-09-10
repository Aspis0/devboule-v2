import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const cssPath = join(dirname(fileURLToPath(import.meta.url)), "design.css");
const css = readFileSync(cssPath, "utf8");

function blockFor(selector: string): string {
  const start = css.indexOf(selector);
  if (start < 0) throw new Error(`Selector missing: ${selector}`);
  const open = css.indexOf("{", start);
  const close = css.indexOf("}", open);
  if (open < 0 || close < 0) throw new Error(`Block missing: ${selector}`);
  return css.slice(open + 1, close);
}

/** All blocks for a selector that appears in a shared rule and its own rule. */
function blocksFor(selector: string): string[] {
  const blocks: string[] = [];
  let from = 0;
  for (;;) {
    const start = css.indexOf(selector, from);
    if (start < 0) break;
    const open = css.indexOf("{", start);
    const close = open < 0 ? -1 : css.indexOf("}", open);
    if (open < 0 || close < 0) break;
    blocks.push(css.slice(open + 1, close));
    from = close + 1;
  }
  if (blocks.length === 0) throw new Error(`Selector missing: ${selector}`);
  return blocks;
}

describe("design message source pills", () => {
  it("lets a long path wrap inside its pill instead of overflowing", () => {
    const block = blockFor(".design-message-source");
    expect(block).toContain("min-height: 20px");
    expect(block).toContain("height: auto");
    expect(block).toContain("overflow-wrap: anywhere");
    // Fixed "height: 20px" clips a wrapped path; "min-height" keeps the
    // single-line size. Match the property at line start so min-height itself
    // does not count.
    expect(block).not.toContain("\n  height: 20px");
  });

  it("applies the same wrap fix to the sources list pills", () => {
    const block = blockFor(".design-message-sources span");
    expect(block).toContain("min-height: 20px");
    expect(block).toContain("height: auto");
    expect(block).toContain("overflow-wrap: anywhere");
    expect(block).not.toContain("\n  height: 20px");
  });
});

describe("design inspector panel", () => {
  it("caps its height to the canvas and scrolls instead of clipping", () => {
    // .design-inspector-panel shares one rule with .design-layers-panel and
    // owns a second rule with its top/right/width; the cap lives in the own rule.
    const own = blocksFor(".design-inspector-panel").find((block) => block.includes("top: 14px"));
    if (own === undefined) throw new Error("Inspector own rule missing");
    expect(own).toContain("max-height:");
    expect(own).toContain("100%");
    expect(own).toMatch(/overflow-y:\s*auto/);
  });
});
