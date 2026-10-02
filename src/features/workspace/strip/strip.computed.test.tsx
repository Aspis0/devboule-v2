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
  const { inject, rulesFor, token } = assembleCssProof([
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

  it("gives the label the spec's width: the chip's full max-w share", async () => {
    // The spec's arithmetic, every number read off the real sheet: the
    // chip is max-w 160, pad 0/8, gap 6, and holds dot 6 + mark 14 +
    // label. The dot and the mark are flex: none, so they keep their
    // widths at every chip width and the label takes everything the
    // chrome leaves: 160 − 16 (padding) − 6 (dot) − 6 (gap) − 14 (mark)
    // − 6 (gap) = 112 px at the max. happy-dom computes no layout, so
    // this pins the declarations the share depends on; the live window
    // judges the pixels.
    inject([
      ".workspace-session-tab",
      ".workspace-tab-label",
      ".workspace-session-tab .workspace-status-dot",
      ".strip-kind",
    ]);
    await renderWorkspace();
    const chip = document.querySelector<HTMLElement>(".workspace-session-tab");
    if (chip === null) throw new Error("session tab did not render");
    const chipStyle = getComputedStyle(chip);
    const label = chip.querySelector<HTMLElement>(".workspace-tab-label");
    if (label === null) throw new Error("tab label did not render");
    const labelStyle = getComputedStyle(label);
    const dot = chip.querySelector<HTMLElement>(".workspace-status-dot");
    if (dot === null) throw new Error("tab dot did not render");
    const dotStyle = getComputedStyle(dot);
    const mark = chip.querySelector<HTMLElement>(".strip-kind");
    if (mark === null) throw new Error("tab kind mark did not render");
    const markStyle = getComputedStyle(mark);
    // The label carries no width of its own: it takes everything the
    // chrome leaves. happy-dom reads an unspecified max-width as "".
    expect(labelStyle.minWidth).toBe("0");
    expect(labelStyle.maxWidth).toBe("");
    // The dot and the mark must not shrink: the label is the only
    // shrinkable item, so the spec's arithmetic holds at every width.
    expect(dotStyle.flexShrink).toBe("0");
    expect(markStyle.flexShrink).toBe("0");
    // The share, computed from the sheet's own values.
    const px = (value: string): number => Number.parseFloat(value);
    const share =
      px(chipStyle.maxWidth) -
      px(chipStyle.paddingLeft) -
      px(chipStyle.paddingRight) -
      px(chipStyle.gap) -
      px(dotStyle.width) -
      px(chipStyle.gap) -
      px(markStyle.width);
    expect(share).toBe(112);
  });

  it("reserves the ×'s 16 px and fades the label's tail under it, on hover only", () => {
    // The mockup's values, on the real sheet: the hovered label gets
    // pad-right 16 — the ×'s reservation, so it never sits on the
    // label's last letters — and a 22 px mask, so the tail dissolves
    // into the scrim instead of ending under the ×. Focus reveals the ×
    // without the reservation: arrow keys move real focus, and a focus
    // reservation would grow the chip and shift the strip on every
    // press. Selection gets neither.
    const hoverLabel = rulesFor(".workspace-session-row:hover .workspace-tab-label");
    expect(hoverLabel).toContain("padding-right: 16px");
    expect(hoverLabel).toContain("calc(100% - 22px)");
    const selectedLabel = rulesFor(
      ".workspace-session-row:has(.workspace-session-tab-selected) .workspace-tab-label",
    );
    expect(selectedLabel).toBe("");
    // The × shows on hover, on keyboard focus, and on a tap's
    // focus-within, over its scrim.
    const hoverChip = rulesFor(".workspace-session-row:hover .workspace-session-chip");
    expect(hoverChip).toContain("opacity: 1");
    expect(hoverChip).toContain("visibility: visible");
    const selectedChip = rulesFor(
      ".workspace-session-row:has(.workspace-session-tab-selected) .workspace-session-chip",
    );
    expect(selectedChip).toBe("");
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

  it("holds the overview's selected option on the selection tokens", () => {
    const selected = rulesFor('.workspace-overview-option[aria-selected="true"]');
    expect(selected).not.toBe("");
    expect(selected).toContain(`color: ${token("--ink")}`);
    expect(selected).toContain(`background: ${token("--fill-selected")}`);
  });
});
