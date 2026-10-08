import { describe, expect, it } from "vitest";
import { INITIAL_RIGHT_WIDTH, MAX_RIGHT_WIDTH, MIN_RIGHT_WIDTH } from "../workspaceResize";
import { PANEL_TAB_LABEL_MIN_WIDTH, panelTabsShowLabels } from "./panelTabLabels";

describe("the side panel's tab row label budget", () => {
  it("goes icon-only at the default width, where the four labels do not fit", () => {
    expect(panelTabsShowLabels(INITIAL_RIGHT_WIDTH)).toBe(false);
    expect(PANEL_TAB_LABEL_MIN_WIDTH).toBeGreaterThan(INITIAL_RIGHT_WIDTH);
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

  it("sits at the width the four labelled tabs measure in the app", () => {
    // Tab widths measured live, the three gaps between four tabs, the kebab,
    // and the row's padding and gaps: 283 + 6 + 24 + 20.
    const tabs = 59 + 85 + 73 + 66;
    const gaps = 3 * 2;
    const kebab = 24;
    const rowChrome = 8 * 2 + 2 * 2;
    expect(tabs + gaps + kebab + rowChrome).toBe(PANEL_TAB_LABEL_MIN_WIDTH);
  });
});
