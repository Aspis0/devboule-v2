// @vitest-environment happy-dom

// The split layout itself: what the centre panel renders with a split and
// without one, the divider as a keyboard-operable separator, and the merge
// control in the lower pane's header. happy-dom lays nothing out, so the
// divider's geometry is asserted through the module that answers it, and the
// two rules only a stylesheet can carry — the compact threshold and the
// control's reveal on hover and focus — are read off the sheet itself.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SplitPane } from "./SplitPane";
import {
  COMPACT_MAX_HEIGHT,
  COMPACT_MAX_WIDTH,
  DIVIDER_PX,
  DEFAULT_SPLIT_SIZE,
  MAX_SPLIT_SIZE,
  MIN_BOTTOM_PANE_PX,
  MIN_SPLIT_SIZE,
  MIN_TOP_PANE_PX,
  splitBoundsFor,
} from "./splitGeometry";

const sheet = readFileSync(resolve(import.meta.dirname, "SplitPane.css"), "utf8");

/** The rule body a selector owns, for what jsdom cannot compute. */
function ruleBody(selector: string): string {
  const at = sheet.indexOf(`${selector} {`);
  expect(at, `${selector} has no rule`).toBeGreaterThan(-1);
  return sheet.slice(at, sheet.indexOf("}", at));
}

/** The rule body a selector owns inside a larger sheet (the compact block). */
function bodyOf(css: string, selector: string): string {
  const at = css.indexOf(`${selector} {`);
  expect(at, `${selector} has no rule`).toBeGreaterThan(-1);
  return css.slice(at, css.indexOf("}", at));
}

/** The split area's own box, which happy-dom cannot lay out. */
function stubArea(height: number): void {
  const area = container.querySelector<HTMLElement>(".workspace-split");
  if (area === null) throw new Error("the split did not render");
  vi.spyOn(area, "getBoundingClientRect").mockReturnValue({
    top: 0,
    left: 0,
    width: 600,
    height,
    right: 600,
    bottom: height,
    toJSON: () => ({}),
  } as DOMRect);
}

/** One window resize, which is what tells a laid-out split area it moved. */
async function areaMeasured(): Promise<void> {
  await act(async () => {
    window.dispatchEvent(new Event("resize"));
  });
}

/** What the component would write for a share: the same three decimals the
 * inline style carries, so a test cannot pass on a rounded number the browser
 * would then render. */
function splitSizeAtPane(size: number): string {
  return `${Number((size * 100).toFixed(3))}%`;
}

function boundsOf(height: number): { min: number; max: number } {
  return splitBoundsFor(height);
}

const LOWER_TAB = "tool:browser:a:page-1";

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
const onResize = vi.fn();
const onMerge = vi.fn();

function render(props: { split: { size: number; lowerTabId: string } | null }): void {
  act(() => {
    root.render(
      <SplitPane
        split={props.split}
        onResize={onResize}
        onMerge={onMerge}
        lowerLabel="example.test"
        lower={<div lower-pane="true">page</div>}
      >
        <p>chat</p>
      </SplitPane>,
    );
  });
}

function splitAt(size = 0.5): { size: number; lowerTabId: string } {
  return { size, lowerTabId: LOWER_TAB };
}

function separator(): HTMLElement {
  const found = container.querySelector<HTMLElement>('[role="separator"]');
  if (found === null) throw new Error("the split did not render a separator");
  return found;
}

describe("a workspace with a split", () => {
  beforeEach(() => {
    onResize.mockReset();
    onMerge.mockReset();
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it("renders the workspace's own pane and nothing else while there is no split", () => {
    render({ split: null });
    expect(container.querySelector(".workspace-split")).toBeNull();
    expect(container.querySelector('[role="separator"]')).toBeNull();
    // The two panes render in the order they are given, so the tab strip's
    // element order is unchanged by a feature that is off.
    expect(container.textContent).toBe("chat");
  });

  it("puts the workspace's pane on top and the split tab below it", () => {
    render({ split: splitAt(0.42) });
    const panes = [...container.querySelectorAll<HTMLElement>(".workspace-split-pane")];
    expect(panes.map((pane) => pane.getAttribute("data-pane"))).toEqual(["top", "bottom"]);
    expect(panes[0]?.textContent).toBe("chat");
    expect(panes[1]?.textContent).toContain("page");
    // The share is written at the geometry's precision, not rounded to a whole
    // percent: a rounded one hands the pane below the pixels the floor took.
    expect(panes[0]?.style.height).toBe("42%");
    expect(splitSizeAtPane(boundsOf(700).min)).toBe("25.714%");
    // The divider owns the height: the pane below takes what the top leaves.
    expect(ruleBody(".workspace-split-top")).toMatch(/flex:\s*none/);
    expect(ruleBody(".workspace-split-bottom")).toMatch(/flex:\s*1 1 0/);
  });

  it("gives the divider a separator that says where it stands", () => {
    render({ split: splitAt(0.42) });
    const bar = separator();
    expect(bar.getAttribute("aria-orientation")).toBe("horizontal");
    expect(bar.getAttribute("aria-valuenow")).toBe("42");
    // An area happy-dom cannot measure leaves the fraction bounds standing.
    expect(bar.getAttribute("aria-valuemin")).toBe(String(Math.round(MIN_SPLIT_SIZE * 100)));
    expect(bar.getAttribute("aria-valuemax")).toBe(String(Math.round(MAX_SPLIT_SIZE * 100)));
    expect(bar.tabIndex).toBe(0);
    expect(bar.getAttribute("aria-label")).toBe("Resize panes");
  });

  it("advertises the bounds the divider can really reach in the area it has", async () => {
    render({ split: splitAt(0.5) });
    stubArea(700);
    await areaMeasured();
    // 700 px: the top pane's 180 px floor is 25.7%, the lower pane's 192 px
    // leaves 72.6% — so the advertised bounds are the pixels, not 20/80.
    const bounds = splitBoundsFor(700);
    expect(separator().getAttribute("aria-valuemin")).toBe(String(Math.round(bounds.min * 100)));
    expect(separator().getAttribute("aria-valuemax")).toBe(String(Math.round(bounds.max * 100)));
    expect(bounds.min).toBeCloseTo(MIN_TOP_PANE_PX / 700, 5);
  });

  it("holds a size the divider would have crushed, when the window is too short for both floors", async () => {
    // A size left by a taller window: 20% of 300 px is a pane with no page in it.
    render({ split: splitAt(0.2) });
    stubArea(300);
    await areaMeasured();
    const bounds = splitBoundsFor(300);
    expect(separator().getAttribute("aria-valuenow")).toBe(String(Math.round(bounds.min * 100)));
    expect(splitSizeAtPane(bounds.min)).toBe("34.333%");
    // The lower pane keeps its floor (plus the divider's band, which is a row
    // of the split too); the upper one takes what is left.
    expect((1 - bounds.min) * 300).toBeCloseTo(MIN_BOTTOM_PANE_PX + DIVIDER_PX, 5);
    // The top pane is what is left of a 300px area, which is under its own
    // floor: the floor the stylesheet no longer repeats is the only answer.
    expect(bounds.min * 300).toBeLessThan(MIN_TOP_PANE_PX);
  });

  it("moves the divider with the arrow keys and hands the size on", () => {
    render({ split: splitAt(0.5) });
    act(() => {
      separator().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(onResize).toHaveBeenCalledWith(expect.closeTo(0.52, 5));
    act(() => {
      separator().dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true }));
    });
    // Home is the separator's minimum value: the top pane's smallest share.
    expect(onResize).toHaveBeenLastCalledWith(MIN_SPLIT_SIZE);
    act(() => {
      separator().dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    });
    expect(onResize).toHaveBeenLastCalledWith(MAX_SPLIT_SIZE);
  });

  it("leaves a key it does not use to the rest of the app", () => {
    render({ split: splitAt() });
    const event = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    act(() => {
      separator().dispatchEvent(event);
    });
    expect(onResize).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
  });

  it("drags the divider with a pointer, and commits the size once at the end", () => {
    render({ split: splitAt(0.5) });
    // The split area's own box is what a pointer row is read against;
    // happy-dom lays nothing out, so it is the one box this needs.
    const area = container.querySelector<HTMLElement>(".workspace-split");
    if (area === null) throw new Error("the split did not render");
    vi.spyOn(area, "getBoundingClientRect").mockReturnValue({
      top: 100,
      left: 0,
      width: 600,
      height: 800,
      right: 600,
      bottom: 900,
      toJSON: () => ({}),
    } as DOMRect);

    act(() => {
      separator().dispatchEvent(new PointerEvent("pointerdown", { clientY: 500, bubbles: true }));
    });
    act(() => {
      window.dispatchEvent(new PointerEvent("pointermove", { clientY: 420, bubbles: true }));
      window.dispatchEvent(new PointerEvent("pointermove", { clientY: 300, bubbles: true }));
    });
    // The pane follows the pointer through the drag's own state, so the store
    // is written once, at the end.
    expect(onResize).not.toHaveBeenCalled();
    expect(separator().getAttribute("aria-valuenow")).toBe("25");

    act(() => {
      window.dispatchEvent(new PointerEvent("pointerup", { clientY: 300, bubbles: true }));
    });
    expect(onResize).toHaveBeenCalledTimes(1);
    expect(onResize).toHaveBeenCalledWith(expect.closeTo(0.25, 5));
    vi.restoreAllMocks();
  });

  it("gives a drag up before the pointer does not move a divider", () => {
    render({ split: splitAt(0.5) });
    act(() => {
      separator().dispatchEvent(new PointerEvent("pointerdown", { clientY: 500, bubbles: true }));
      window.dispatchEvent(new PointerEvent("pointerup", { clientY: 500, bubbles: true }));
      window.dispatchEvent(new PointerEvent("pointermove", { clientY: 100, bubbles: true }));
    });
    expect(onResize).not.toHaveBeenCalled();
  });

  it("carries the merge control in the lower pane's header, and it acts", () => {
    render({ split: splitAt() });
    const header = container.querySelector<HTMLElement>(".workspace-split-header");
    expect(header?.textContent).toContain("example.test");
    const merge = header?.querySelector<HTMLButtonElement>(".workspace-split-merge");
    expect(merge?.type).toBe("button");
    expect(merge?.textContent).toBe("Merge into tabs");
    expect(merge?.closest(".workspace-split-pane")?.getAttribute("data-pane")).toBe("bottom");
    act(() => merge?.click());
    expect(onMerge).toHaveBeenCalled();
  });

  it("keeps the merge control visible at all times, not only on hover", () => {
    // A control that appears only under the pointer hides the way back.
    const base = ruleBody(".workspace-split-merge");
    expect(base).not.toMatch(/opacity:\s*0/);
    expect(base).not.toMatch(/transition:[^;]*opacity/);
    expect(base).not.toMatch(/display:\s*none|visibility:\s*hidden/);
  });

  it("keeps the merge control at least 24 px tall and named for a screen reader", () => {
    const base = ruleBody(".workspace-split-merge");
    expect(base).toMatch(/min-height:\s*24px/);
    // Hidden, never removed: a keyboard that arrives at it must find it, and it
    // must carry a name without help from a tooltip.
    render({ split: splitAt() });
    const merge = container.querySelector<HTMLButtonElement>(".workspace-split-merge");
    expect(merge?.disabled).toBe(false);
    expect(merge?.tabIndex).toBe(0);
    expect((merge?.textContent ?? "").trim()).toBe("Merge into tabs");
  });
});

describe("a pane too small to read at full size", () => {
  it("steps the interface type down one notch below about half a workspace", () => {
    const query = sheet.slice(sheet.indexOf("@container"));
    expect(query).toContain(`(max-height: ${COMPACT_MAX_HEIGHT}px)`);
    expect(query).toContain(`(max-width: ${COMPACT_MAX_WIDTH}px)`);
    // A pane above the threshold keeps the full-size rows; the compact block
    // only adds rules under its own condition.
    expect(ruleBody(".workspace-split-header")).toMatch(
      new RegExp(`height:\\s*var\\(--control-bar\\)`),
    );
    expect(query).toMatch(/height:\s*var\(--control-dense\)/);
    expect(query).toMatch(/font-size:\s*var\(--type-small\)/);
    // The pane above compacts too: the terminal's own header row is named here.
    expect(query).toMatch(/\.workspace-split-pane \.workspace-terminal-toolbar/);
    // 13 px is the step below the 14 px interface: the 12 px meta floor holds.
    expect(query).not.toMatch(/font-size:\s*var\(--type-meta\)/);
  });

  it("compacts the transcript and the composer, not only the headers around them", () => {
    const query = sheet.slice(sheet.indexOf("@container"));
    const entry = bodyOf(query, ".workspace-split-pane .workspace-chat-entry");
    expect(entry).toMatch(/font-size:\s*var\(--type-small\)/);
    expect(entry).toMatch(/line-height:\s*var\(--leading-dense\)/);
    // The rows sit a step closer together, and the composer gives back its
    // padding: that is what a short middle pane is paying for.
    expect(bodyOf(query, ".workspace-split-pane .workspace-conversation-content")).toMatch(
      /gap:\s*var\(--space-6\)/,
    );
    const composer = bodyOf(query, ".workspace-split-pane .workspace-composer");
    expect(composer).toMatch(/padding:\s*var\(--space-10\) var\(--space-10\) var\(--space-6\)/);
    expect(composer).not.toMatch(/min-height/);
    expect(bodyOf(query, ".workspace-split-pane .workspace-composer textarea")).toMatch(
      /font-size:\s*var\(--type-small\)/,
    );
    expect(bodyOf(query, ".workspace-split-pane .workspace-composer-wrap")).toMatch(
      /padding:\s*var\(--space-4\) var\(--space-24\) var\(--space-8\)/,
    );
  });

  it("queries the pane's own box, so a wide workspace and a short one both compact", () => {
    expect(ruleBody(".workspace-split-pane")).toMatch(/container-type:\s*size/);
    expect(ruleBody(".workspace-split-pane")).toMatch(/container-name:\s*split-pane/);
  });

  it("keeps the default size inside the bounds the divider enforces", () => {
    expect(DEFAULT_SPLIT_SIZE).toBeGreaterThan(MIN_SPLIT_SIZE);
    expect(DEFAULT_SPLIT_SIZE).toBeLessThan(MAX_SPLIT_SIZE);
  });

  it("leaves the floor to the geometry, so the two cannot disagree", () => {
    // A `min-height` here would be a second answer to a question splitGeometry
    // already answers, and the one the stylesheet would win: at a 300px area
    // the geometry hands the pane above 108px and this floor would take 180.
    expect(ruleBody(".workspace-split-top")).not.toMatch(/min-height/);
    expect(ruleBody(".workspace-split-bottom")).not.toMatch(/min-height/);
    // What the stylesheet does own is that the three rows add up to the split's
    // own height, so nothing at this level can overflow.
    expect(ruleBody(".workspace-split-top")).toMatch(/flex:\s*none/);
    expect(ruleBody(".workspace-split-bottom")).toMatch(/flex:\s*1 1 0/);
    expect(ruleBody(".workspace-split-divider")).toMatch(/flex:\s*none/);
    // The divider's band is a row of the split and the lower pane's floor
    // counts it, so the two numbers are pinned to each other here.
    expect(ruleBody(".workspace-split-divider")).toContain(`height: ${DIVIDER_PX}px`);
    // A pane clips what does not fit and lets its own scrollport scroll: the
    // floor is the geometry's, the overflow is the pane's.
    expect(ruleBody(".workspace-split-pane")).toMatch(/overflow:\s*hidden/);
  });

  it("has the first measurement land before the first paint, so no frame is drawn unmeasured", () => {
    const source = readFileSync(resolve(import.meta.dirname, "SplitPane.tsx"), "utf8");
    // A useEffect here would paint one frame at the unmeasured fraction bounds,
    // which is what the CSS floor used to cover up.
    expect(source).toMatch(/useLayoutEffect\(\(\) => \{\s*const area = areaRef\.current/);
    expect(source).not.toMatch(/useEffect\(\(\) => \{\s*const area = areaRef\.current/);
  });
});
