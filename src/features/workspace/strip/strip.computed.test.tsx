// @vitest-environment happy-dom

// The strip's geometry against the real stylesheets: the assembled sheets in
// bundle order (strip before workspace, as the module graph bundles them),
// so a cascade inversion like the "+" losing its fill fails here, not live.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { beforeEachHarness, renderWorkspace, unmountWorkspace } from "../bulkCloseHarness";
import { assembleCssProof, removeCssProof } from "../cssProof";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await unmountWorkspace();
  removeCssProof();
});

describe("strip computed styles (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle: tokens, global, strip (pulled in by
  // SessionStrip, which Workspace imports before its own CSS), workspace.
  const { inject, token } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/workspace/strip/strip.css"),
    read("src/features/workspace/Workspace.css"),
  ]);

  it("the tab strip is a flex row again, not a stacked block", async () => {
    inject([".workspace-session-tabs"]);
    await renderWorkspace();
    const strip = document.querySelector<HTMLElement>(".workspace-session-tabs");
    if (strip === null) throw new Error("session tab strip did not render");
    const style = getComputedStyle(strip);
    expect(style.display).toBe("flex");
    expect(style.height).toBe("36px");
  });

  it("the tab chip and the add button keep the spec geometry", async () => {
    inject([".workspace-session-tab", ".workspace-session-add", ".workspace-tab-label"]);
    await renderWorkspace();
    const chip = document.querySelector<HTMLElement>(".workspace-session-tab");
    if (chip === null) throw new Error("session tab did not render");
    const chipStyle = getComputedStyle(chip);
    expect(chipStyle.height).toBe("28px");
    expect(chipStyle.minWidth).toBe("96px");
    expect(chipStyle.maxWidth).toBe("160px");
    expect(chipStyle.borderRadius).toBe("6px");
    expect(chipStyle.paddingTop).toBe("0px");
    expect(chipStyle.paddingRight).toBe("8px");
    const label = chip.querySelector<HTMLElement>(".workspace-tab-label");
    if (label === null) throw new Error("tab label did not render");
    expect(getComputedStyle(label).overflow).toBe("hidden");
    const add = document.querySelector<HTMLElement>(".workspace-session-add");
    if (add === null) throw new Error("session add did not render");
    const addStyle = getComputedStyle(add);
    expect(addStyle.width).toBe("28px");
    expect(addStyle.height).toBe("28px");
    expect(addStyle.borderRadius).toBe("6px");
  });

  it("paints the add button from the strip's own rule on the assembled sheets", async () => {
    inject([".workspace-session-add"]);
    await renderWorkspace();
    const add = document.querySelector<HTMLElement>(".workspace-session-add");
    if (add === null) throw new Error("session add did not render");
    const style = getComputedStyle(add);
    // The strip's own fill and ink: the shared base group must not
    // override them, whatever the bundle order does. Expected values come
    // from the sheets, so a theme retune moves the test with the tokens.
    // (happy-dom reports the specified value, not a normalised rgb().)
    expect(style.backgroundColor).toBe(token("--fill-plus"));
    expect(style.color).toBe(token("--ink"));
    // The layout the shared group used to lend: the strip's rule carries
    // it all, so a second removal cannot silently uncentre the glyph.
    expect(style.display).toBe("grid");
    expect(style.placeItems).toBe("center");
    expect(style.paddingTop).toBe("0px");
    expect(style.cursor).toBe("pointer");
    expect(style.flexShrink).toBe("0");
  });
});
