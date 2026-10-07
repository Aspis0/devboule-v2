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
  it.each(["light", "dark"] as const)(
    "draws a 2px --line-strong top edge in the %s theme",
    (theme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([".workspace-status-bar"]);
      const bar = document.createElement("div");
      bar.className = "workspace-status-bar";
      document.body.append(bar);

      const style = getComputedStyle(bar);
      expect(style.borderTopWidth).toBe("2px");
      expect(style.borderTopColor).toBe(css.token("--line-strong"));
      expect(style.height).toBe("26px");
      bar.remove();
    },
  );

  it("fills its meters with the accent, which clears the track at 3:1 or more", () => {
    const css = assembleCssProof(sheets, "light");
    css.inject([".status-meter-fill"]);
    const fill = document.createElement("span");
    fill.className = "status-meter-fill";
    document.body.append(fill);

    expect(getComputedStyle(fill).backgroundColor).toBe(css.token("--accent"));
    expect(css.rulesFor(".status-meter-fill")).not.toContain("--control-on");
    fill.remove();
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
