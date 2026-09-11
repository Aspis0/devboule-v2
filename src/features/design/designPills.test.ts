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

describe("design layers panel", () => {
  it("scrolls its list inside the canvas instead of outgrowing it", () => {
    const block = blockFor(".design-layer-list");
    expect(block).toMatch(/overflow-y:\s*auto/);
  });

  it("never compresses rows: an expanded row pushes the list into scrolling", () => {
    // A column flex container shrinks the default `flex: 0 1 auto` item to
    // fit, so an expanded row's details overflow onto the next row instead
    // of scrolling. flex:none keeps every row at its natural height.
    const block = blockFor(".design-layer-row");
    expect(block).toMatch(/flex:\s*none/);
  });

  it("caps the panel well below full canvas height", () => {
    // About half a 704px canvas, and never more than the bottom offset +
    // top margin allow. The scrolling list inside absorbs the rest.
    expect(css).toContain("max-height: min(352px, calc(100% - 54px))");
  });
});
