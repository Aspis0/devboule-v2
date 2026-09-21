// Source test, not a layout test. It reads the declarations that make the
// one-character-per-line defect impossible and fails when one is deleted; it
// cannot see a squeezed row. jsdom computes no layout, which is why no test in
// this suite ever caught it, and why the photographs stay the proof:
// scout/user-pass/h6-live-nofix-zoom.png (broken, byte-for-byte the user's
// zoom-history.png) against scout/user-pass/h11-fix-zoom.png (fixed).
//
// Measured in the app's WebView2 with the sidebar at 252 px, so the row's inner
// width is 210 px: the two actions are `flex: none` and never shrink, a live
// row's disabled delete label is 248 px wide, and the copy — the row's only
// shrinkable child — was therefore left at 0 px. Its 384 px meta, which
// `.history-row-meta` deliberately lets wrap mid-token, then came out one
// character per line, 785 px tall, while the action overflowed the row sideways
// by 30 px and grew the panel's horizontal scrollbar.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./history.css", import.meta.url), "utf8");

/** The body of the top-level rule whose selector is exactly `selector`. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`, "m"))?.[1];
  if (body === undefined) throw new Error(`no rule in history.css for ${selector}`);
  return body;
}

describe("a history row", () => {
  it("wraps rather than letting the actions starve the copy", () => {
    expect(ruleBody(".history-row")).toContain("flex-wrap: wrap;");
    // Not to be bought back by deleting this: the copy is the child that gives
    // way when a row overflows, and a floor here would push the overflow back
    // onto the actions instead.
    expect(ruleBody(".history-row-copy")).toContain("min-width: 0;");
  });

  it("holds the archive-first sentence inside the row", () => {
    const action = ruleBody(".history-delete-action");
    // `nowrap` made the 248 px explanation a box the 210 px row had to clip.
    expect(action).toContain("white-space: normal;");
    // The action is `flex: none`, so it will not shrink on its own; without the
    // cap the wrapped sentence still scrolled the row sideways by 22 px.
    expect(action).toContain("max-width: 100%;");
  });
});
