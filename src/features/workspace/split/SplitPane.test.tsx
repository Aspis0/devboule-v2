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
  MAX_SPLIT_SIZE,
  MIN_SPLIT_SIZE,
  DEFAULT_SPLIT_SIZE,
} from "./splitGeometry";

const sheet = readFileSync(resolve(import.meta.dirname, "SplitPane.css"), "utf8");

/** The rule body a selector owns, for what jsdom cannot compute. */
function ruleBody(selector: string): string {
  const at = sheet.indexOf(`${selector} {`);
  expect(at, `${selector} has no rule`).toBeGreaterThan(-1);
  return sheet.slice(at, sheet.indexOf("}", at));
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
    expect(panes[0]?.style.height).toBe("42%");
    // The divider owns the height: the pane below takes what the top leaves.
    expect(ruleBody(".workspace-split-top")).toMatch(/flex:\s*none/);
    expect(ruleBody(".workspace-split-bottom")).toMatch(/flex:\s*1 1 0/);
  });

  it("gives the divider a separator that says where it stands", () => {
    render({ split: splitAt(0.42) });
    const bar = separator();
    expect(bar.getAttribute("aria-orientation")).toBe("horizontal");
    expect(bar.getAttribute("aria-valuenow")).toBe("42");
    expect(bar.getAttribute("aria-valuemin")).toBe(String(Math.round(MIN_SPLIT_SIZE * 100)));
    expect(bar.getAttribute("aria-valuemax")).toBe(String(Math.round(MAX_SPLIT_SIZE * 100)));
    expect(bar.tabIndex).toBe(0);
    expect(bar.getAttribute("aria-label")).toBe("Resize panes");
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

  it("shows the merge control on hover and on keyboard focus, and hides it in neither", () => {
    const base = ruleBody(".workspace-split-merge");
    expect(base).toMatch(/opacity:\s*0/);
    expect(base).toMatch(/transition:[^;]*opacity/);
    const revealed = sheet.slice(sheet.indexOf(".workspace-split-merge:hover"));
    expect(revealed.slice(0, revealed.indexOf("}"))).toMatch(/opacity:\s*1/);
    const focus = sheet.slice(sheet.indexOf(".workspace-split-merge:focus-visible"));
    expect(focus.slice(0, focus.indexOf("}"))).toMatch(/opacity:\s*1/);
    // Hidden, never removed: a keyboard that arrives at it must find it.
    render({ split: splitAt() });
    const merge = container.querySelector<HTMLButtonElement>(".workspace-split-merge");
    expect(merge?.disabled).toBe(false);
    expect(merge?.tabIndex).toBe(0);
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
    // The pane above compacts too: the chat's own header row is named here.
    expect(query).toMatch(/\.workspace-split-pane \.workspace-agent-toolbar/);
    // 13 px is the step below the 14 px interface: the 12 px meta floor holds.
    expect(query).not.toMatch(/font-size:\s*var\(--type-meta\)/);
  });

  it("queries the pane's own box, so a wide workspace and a short one both compact", () => {
    expect(ruleBody(".workspace-split-pane")).toMatch(/container-type:\s*size/);
    expect(ruleBody(".workspace-split-pane")).toMatch(/container-name:\s*split-pane/);
  });

  it("keeps the default size inside the bounds the divider enforces", () => {
    expect(DEFAULT_SPLIT_SIZE).toBeGreaterThan(MIN_SPLIT_SIZE);
    expect(DEFAULT_SPLIT_SIZE).toBeLessThan(MAX_SPLIT_SIZE);
  });
});
