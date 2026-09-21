// Source test, not a layout test. It reads the declarations that make the
// squeezed-tab defect impossible and fails when one of them is deleted; it cannot
// see a clipped pixel. happy-dom computes no layout, and a programmatic `.click()`
// bypasses hit-testing — which is how `WorkspaceArchive.test.tsx` stayed green
// while Archive and Delete owned none of their own pixels (D8, night field test of
// 18 September). Whether a tab is reachable by pointer stays a manual check.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync(new URL("./Workspace.css", import.meta.url), "utf8");
const tsx = readFileSync(new URL("./Workspace.tsx", import.meta.url), "utf8");

/** The body of the top-level rule whose selector is exactly `selector`. */
function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const body = css.match(new RegExp(`^${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`, "m"))?.[1];
  if (body === undefined) throw new Error(`no rule in Workspace.css for ${selector}`);
  return body;
}

describe("the session strip", () => {
  it("scrolls its tabs instead of squeezing them", () => {
    const scroller = ruleBody(".workspace-session-tabs-scroll");
    expect(scroller).toContain("overflow-x: auto;");
    // A lone `overflow-x: auto` computes the other axis to `auto`, and the row
    // then grows a vertical bar beside 29 px tabs.
    expect(scroller).toContain("overflow-y: hidden;");
  });

  it("forbids the row from shrinking a tab below its content", () => {
    expect(ruleBody(".session-swipe")).toContain("flex: none;");
  });

  it("keeps the clip the swipe reveal depends on", () => {
    // Not to be bought back by deleting `overflow: hidden`: the archive and delete
    // underlays are revealed by sliding the tab over them.
    expect(ruleBody(".session-swipe")).toContain("overflow: hidden;");
  });

  it("stays a single row", () => {
    // Wrapping would put a second 44 px row of tabs over the panel's content.
    expect(ruleBody(".workspace-session-tabs")).not.toContain("flex-wrap: wrap");
  });

  it("leaves the add button outside the box that scrolls", () => {
    const scrollerAt = tsx.indexOf("workspace-session-tabs-scroll");
    expect(scrollerAt).toBeGreaterThan(-1);
    const openEnd = tsx.indexOf(">", scrollerAt) + 1;
    const nextBoxAt = tsx.indexOf("<div", openEnd);
    const inside = tsx.slice(openEnd, nextBoxAt);
    expect(inside).toContain("{visibleSessions.map(");
    // The scrollport closes before any other box opens — the tabs inside it open
    // none — and the next box to open is the add button's own wrapper: the button
    // is the scroller's sibling, not a passenger.
    expect(inside).toContain("</div>");
    expect(inside.match(/<div/g) ?? []).toHaveLength(0);
    expect(tsx.slice(nextBoxAt, tsx.indexOf(">", nextBoxAt))).toContain(
      "workspace-session-add-wrap",
    );
  });
});
