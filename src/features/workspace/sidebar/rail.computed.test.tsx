// @vitest-environment happy-dom

// The rail's hierarchy through the real stylesheets: the selection is a neutral
// fill with ochre only as a thin marker on its edge, and the second line is
// quiet and mono.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");
const sheets = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/sidebar/sidebar.css"), "utf8"),
];

afterEach(() => {
  removeCssProof();
  document.documentElement.removeAttribute("data-theme");
});

function el(tag: string, className: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

describe("the rail's hierarchy", () => {
  it.each(["light", "dark"] as const)(
    "selects with a neutral fill and a thin ochre marker, never an ochre fill, in the %s theme",
    (theme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([".workspace-row", ".workspace-row-selected"]);
      const workspaceRow = el("button", "workspace-row workspace-row-selected");
      document.body.append(workspaceRow);

      const style = getComputedStyle(workspaceRow);
      expect(style.backgroundColor).toBe(css.token("--fill-selected-soft"));
      expect(style.backgroundColor).not.toBe(css.token("--accent"));
      // The marker: a 2px inset on the leading edge, in the accent.
      expect(style.boxShadow).toBe(`inset 2px 0 0 ${css.token("--accent")}`);
      workspaceRow.remove();
    },
  );

  it("sets the branch in mono and quiet, with the totals at the far edge", () => {
    const css = assembleCssProof(sheets, "light");
    css.inject([
      ".workspace-row-sub",
      ".workspace-row-branch",
      ".workspace-row-totals",
      ".workspace-row-line",
    ]);
    const sub = el("span", "workspace-row-line workspace-row-sub");
    const branch = el("span", "workspace-row-branch", "feat/handoff");
    const totals = el("span", "workspace-row-totals", "+12 −3");
    sub.append(branch, totals);
    document.body.append(sub);

    expect(getComputedStyle(sub).color).toBe(css.token("--muted"));
    expect(getComputedStyle(sub).fontSize).toBe("12px");
    expect(getComputedStyle(branch).fontFamily).toContain("JetBrains Mono");
    expect(getComputedStyle(branch).textOverflow).toBe("ellipsis");
    expect(getComputedStyle(totals).marginLeft).toBe("auto");
    sub.remove();
  });
});
