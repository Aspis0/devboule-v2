import { describe, expect, it } from "vitest";
import { INITIAL_RIGHT_WIDTH, MAX_RIGHT_WIDTH, MIN_RIGHT_WIDTH } from "../workspaceResize";
import { PANEL_TAB_LABEL_MIN_WIDTH, panelTabsShowLabels } from "./panelTabLabels";

describe("the side panel's tab row label budget", () => {
  it("shows the labels at the panel's default width, where they fit", () => {
    expect(panelTabsShowLabels(INITIAL_RIGHT_WIDTH)).toBe(true);
    expect(PANEL_TAB_LABEL_MIN_WIDTH).toBeLessThan(INITIAL_RIGHT_WIDTH);
  });

  it("goes icon-only at the panel's own minimum, where they cannot fit", () => {
    expect(panelTabsShowLabels(MIN_RIGHT_WIDTH)).toBe(false);
    expect(MIN_RIGHT_WIDTH).toBeLessThan(PANEL_TAB_LABEL_MIN_WIDTH);
  });

  it("switches once: the budget is the boundary itself", () => {
    expect(panelTabsShowLabels(PANEL_TAB_LABEL_MIN_WIDTH)).toBe(true);
    expect(panelTabsShowLabels(PANEL_TAB_LABEL_MIN_WIDTH - 1)).toBe(false);
    expect(panelTabsShowLabels(MAX_RIGHT_WIDTH)).toBe(true);
  });

  it("sits above the width the three spec labels measure", () => {
    // The labels and the row chrome around them: 27.45 + 52.05 + 40.47 px of
    // 13px Inter text, three tabs' 30px of padding, icon and gap, the
    // tablist's and the row's gaps and padding, and the 24px kebab. Measured
    // in a browser against these sheets; the constant is that sum rounded up,
    // so a panel name that grows fails here instead of clipping a tab.
    const labels = 27.45 + 52.05 + 40.47;
    const perTab = 12 + 14 + 4;
    const rowChrome = 8 * 2 + 2 * 2 + 24;
    expect(Math.ceil(labels + perTab * 3 + 2 * 2 + rowChrome)).toBe(PANEL_TAB_LABEL_MIN_WIDTH);
  });
});
