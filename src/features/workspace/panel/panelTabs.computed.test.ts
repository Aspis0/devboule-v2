// @vitest-environment happy-dom

// The side panel's tab row against the live sheets. The tablist clips its
// overflow, so the app's focus ring — drawn outside the control — keeps only
// the one edge that faces the tablist's own start. The tab answers inside its
// own box instead; :focus-visible cannot be driven here, so the ring is read
// from the cascade's text.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof, specificity } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");

const sheets = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/styles/global.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/panel/panel.css"), "utf8"),
];

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe.each(["light", "dark"] as const)("side panel tab (live stylesheets, %s)", (theme) => {
  const { inject, rulesFor, token } = assembleCssProof(sheets, theme);

  it("draws the focused tab's ring inside its own box", () => {
    // --ring aliases --accent, so the resolved ring is the accent the rest of
    // the app's in-bounds rings use; only the offset differs from the global.
    const focus = rulesFor(".workspace-panel-tab:focus-visible");
    expect(focus).toContain(`outline: 2px solid ${token("--accent")}`);
    expect(focus).toContain("outline-offset: -2px");
  });

  it("outranks the app-wide ring it would otherwise inherit at 3px", () => {
    // The clip hazard is real because the global rule is outside-drawn; the
    // tab's own rule only helps if it wins the cascade, and it does so on
    // specificity rather than on sheet order.
    expect(rulesFor("button:focus-visible")).toContain("outline-offset: 3px");
    expect(specificity(".workspace-panel-tab:focus-visible")).toEqual([0, 2, 0]);
    expect(specificity("button:focus-visible")).toEqual([0, 1, 1]);
  });

  it("leaves the selected tab's underline on its own token", () => {
    inject([".workspace-panel-tab", ".workspace-panel-tab-active"]);
    const tab = document.createElement("button");
    tab.className = "workspace-panel-tab workspace-panel-tab-active";
    document.body.appendChild(tab);

    expect(getComputedStyle(tab).borderBottomColor).toBe(token("--accent"));
  });
});
