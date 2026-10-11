// @vitest-environment happy-dom

// The bar's stronger top edge, through the real stylesheets: it is the only
// surface in the workspace that draws one.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");
const sheets = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/statusBar/StatusBar.css"), "utf8"),
];

afterEach(() => {
  removeCssProof();
  document.documentElement.removeAttribute("data-theme");
});

describe("the status bar's look", () => {
  it.each(["light", "dark"] as const)("draws a 1px --line top edge in the %s theme", (theme) => {
    const css = assembleCssProof(sheets, theme);
    css.inject([".workspace-status-bar"]);
    const bar = document.createElement("div");
    bar.className = "workspace-status-bar";
    document.body.append(bar);

    const style = getComputedStyle(bar);
    expect(style.borderTopWidth).toBe("1px");
    expect(style.borderTopColor).toBe(css.token("--line"));
    expect(style.height).toBe("26px");
    bar.remove();
  });

  it("fills a meter in the neutral control ink, and in the warn and danger tones past 70 and 90 percent", () => {
    const css = assembleCssProof(sheets, "light");
    css.inject([".status-meter-fill", ".status-meter-fill-warn", ".status-meter-fill-danger"]);
    const fillOf = (className: string) => {
      const fill = document.createElement("span");
      fill.className = className;
      document.body.append(fill);
      return fill;
    };
    // --control-on aliases --ink; token() reads one level, so name the base.
    expect(getComputedStyle(fillOf("status-meter-fill")).backgroundColor).toBe(css.token("--ink"));
    expect(
      getComputedStyle(fillOf("status-meter-fill status-meter-fill-warn")).backgroundColor,
    ).toBe(css.token("--tone-warn"));
    expect(
      getComputedStyle(fillOf("status-meter-fill status-meter-fill-danger")).backgroundColor,
    ).toBe(css.token("--danger"));
    for (const fill of document.querySelectorAll(".status-meter-fill")) fill.remove();
  });

  it("sets the status dot 8px away from the agent's words", () => {
    const css = assembleCssProof(sheets, "light");
    expect(css.rulesFor(".status-bar-who")).toContain("gap: 8px");
  });

  it("clips the title at 28 characters, so it never takes the bar's width", () => {
    const css = assembleCssProof(sheets, "light");
    const rules = css.rulesFor(".status-bar-title");
    expect(rules).toContain("max-width: 28ch");
    expect(rules).toContain("text-overflow: ellipsis");
    expect(rules).toContain("white-space: nowrap");
  });

  it("is a single line that clips rather than wraps", () => {
    const css = assembleCssProof(sheets, "light");
    css.inject([".workspace-status-bar"]);
    const bar = document.createElement("div");
    bar.className = "workspace-status-bar";
    document.body.append(bar);

    const style = getComputedStyle(bar);
    expect(style.whiteSpace).toBe("nowrap");
    expect(style.overflow).toBe("hidden");
    expect(style.fontSize).toBe("12px");
    bar.remove();
  });
});
