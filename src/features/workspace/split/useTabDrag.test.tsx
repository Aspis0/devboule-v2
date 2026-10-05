// @vitest-environment happy-dom

// The gesture, with no workspace around it: a press that travels is a drag, a
// press that does not is a click, the zone follows the pointer, and every way
// out of a drag — Escape, a lost pointer, a hidden window, a tab that closed —
// ends it without acting. The layer below the workspace owns it, so the test
// also counts what a pointer move costs the surface above.

import { Profiler, act, useEffect, useRef } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SplitDragLayer, type SplitDragLayerHandle } from "./SplitDragLayer";
import { DRAG_SLOP_PX, type TabDragBoxes } from "./useTabDrag";
import type { DropZone } from "./tabDropZones";

/** The centre a drop is read against, and the strip above it. */
const CENTER = { left: 0, top: 100, width: 1000, height: 800 };
const STRIP = { left: 0, top: 40, width: 1000, height: 40 };

/** The chip a press starts on: a real element in the document, because the
 * capture events a surface loses are delivered to it and bubble from there. */
const chipCapture = vi.fn();
const chipRelease = vi.fn();
const chipHolds = vi.fn(() => true);
const CHIP = Object.assign(document.createElement("div"), {
  setPointerCapture: chipCapture,
  releasePointerCapture: chipRelease,
  hasPointerCapture: chipHolds,
});

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
let handle: SplitDragLayerHandle | null = null;
let dropped: Array<[string, DropZone | "strip"]> = [];
/** The only kind of tab the pane below may hold, so the only kind that starts
 * a gesture. */
const TAB = "tool:browser:w-1:page-1";
let openTabs = new Set([TAB]);
let commits = 0;
let parentRenders = 0;

function box(rect: { left: number; top: number; width: number; height: number }): DOMRect {
  return {
    ...rect,
    right: rect.left + rect.width,
    bottom: rect.top + rect.height,
    toJSON: () => ({}),
  } as DOMRect;
}

const boxes = (): TabDragBoxes => ({
  area: { getBoundingClientRect: () => box(CENTER) } as HTMLElement,
  strip: { getBoundingClientRect: () => box(STRIP) } as HTMLElement,
});

/** A parent that counts its own renders, and the layer under it that counts the
 * commits the gesture causes. */
function Harness() {
  const ref = useRef<SplitDragLayerHandle>(null);
  // Counted after the render, not during it: what this test measures is how
  // many commits a pointer move costs, and writing a counter during a render
  // would itself be a side effect.
  useEffect(() => {
    parentRenders += 1;
    handle = ref.current;
  });
  return (
    <Profiler
      id="layer"
      onRender={() => {
        commits += 1;
      }}
    >
      <SplitDragLayer
        ref={(value) => {
          ref.current = value;
          handle = value;
        }}
        boxes={boxes}
        hasTab={(tabId) => openTabs.has(tabId)}
        onDrop={(tabId, where) => dropped.push([tabId, where])}
      />
    </Profiler>
  );
}

function press(tabId = TAB, at = { x: 500, y: 60 }): void {
  act(() => {
    handle?.start(tabId, CHIP as unknown as Element, {
      clientX: at.x,
      clientY: at.y,
      pointerId: 1,
    });
  });
}
function move(to: { x: number; y: number }): void {
  act(() => {
    window.dispatchEvent(
      new PointerEvent("pointermove", { clientX: to.x, clientY: to.y, pointerId: 1 }),
    );
  });
}
function release(at: { x: number; y: number }): void {
  act(() => {
    window.dispatchEvent(
      new PointerEvent("pointerup", { clientX: at.x, clientY: at.y, pointerId: 1 }),
    );
  });
}
function travel(to: { x: number; y: number }, from = { x: 500, y: 60 }): void {
  press(TAB, from);
  move({ x: to.x + DRAG_SLOP_PX, y: to.y });
  move(to);
}
function escape(): void {
  act(() => {
    window.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    );
  });
}
function preview(): HTMLElement | null {
  return container.querySelector<HTMLElement>(".workspace-drop-preview");
}
function previewZone(): string | null {
  return preview()?.getAttribute("data-zone") ?? null;
}
function dragging(): boolean {
  return document.body.classList.contains("workspace-is-dragging-tab");
}

beforeEach(() => {
  dropped = [];
  openTabs = new Set([TAB]);
  commits = 0;
  parentRenders = 0;
  handle = null;
  chipCapture.mockReset();
  chipRelease.mockReset();
  chipHolds.mockReset().mockReturnValue(true);
  document.body.append(CHIP);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => root.render(<Harness />));
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  CHIP.remove();
  document.body.classList.remove("workspace-is-dragging-tab");
});

describe("a chip press that travels", () => {
  it("is a drag, and the preview follows the pointer", () => {
    travel({ x: 500, y: 200 }); // the centre's top band
    expect(previewZone()).toBe("top");
    move({ x: 500, y: 900 });
    expect(previewZone()).toBe("bottom");
    release({ x: 500, y: 900 });
    dropped.length = 0;
  });

  it("is nothing at all until it passes the slop", () => {
    press();
    move({ x: 500 + DRAG_SLOP_PX - 1, y: 60 });
    expect(preview()).toBeNull();
    expect(dragging()).toBe(false);
    expect(chipCapture).not.toHaveBeenCalled();

    move({ x: 500 + DRAG_SLOP_PX, y: 60 });
    expect(dragging()).toBe(true);
    release({ x: 500 + DRAG_SLOP_PX, y: 60 });
    dropped.length = 0;
  });

  it("picks up only a tab the pane below can hold", () => {
    // The tab is open, so nothing but the kind of tab can refuse it.
    openTabs = new Set([TAB, "session-1"]);

    press("session-1");
    move({ x: 700, y: 900 });
    expect(dragging()).toBe(false);
    expect(preview()).toBeNull();
    release({ x: 700, y: 900 });
    expect(dropped).toEqual([]);
  });

  it("takes the capture on the chip that owns the gesture", () => {
    travel({ x: 500, y: 900 });
    expect(chipCapture).toHaveBeenCalledWith(1);
    // The element under the pointer later is never the one captured: the owner
    // is the only argument the capture ever carried.
    expect(chipCapture).toHaveBeenCalledTimes(1);
    release({ x: 500, y: 900 });
  });

  it("selects nothing while it is held", () => {
    travel({ x: 500, y: 900 });
    expect(dragging()).toBe(true);
    release({ x: 500, y: 900 });
    expect(dragging()).toBe(false);
    dropped.length = 0;
  });

  it("says strip when the pointer is back over the tab row, and draws nothing there", () => {
    travel({ x: 500, y: 60 }, { x: 500, y: 300 });
    expect(dragging()).toBe(true);
    expect(preview()).toBeNull();
    release({ x: 500, y: 60 });
    expect(dropped).toEqual([[TAB, "strip"]]);
  });

  it("has no preview over neither box", () => {
    travel({ x: 1500, y: 600 });
    expect(preview()).toBeNull();
    release({ x: 1500, y: 600 });
    expect(dropped).toEqual([]);
  });
});

describe("every way out of a drag", () => {
  it("reports the zone under the pointer and hands the pointer back", () => {
    travel({ x: 500, y: 900 });
    release({ x: 500, y: 900 });
    expect(dropped).toEqual([[TAB, "bottom"]]);
    expect(chipRelease).toHaveBeenCalledWith(1);
    expect(preview()).toBeNull();
  });

  it("does nothing at all for a press that never moved", () => {
    press();
    release({ x: 500, y: 60 });
    expect(dropped).toEqual([]);
    expect(chipCapture).not.toHaveBeenCalled();
  });

  it("leaves the tab where it was on Escape, and hands the pointer back", () => {
    travel({ x: 500, y: 900 });
    escape();
    expect(dropped).toEqual([]);
    expect(preview()).toBeNull();
    expect(dragging()).toBe(false);
    expect(chipRelease).toHaveBeenCalledWith(1);
    release({ x: 500, y: 900 });
    expect(dropped).toEqual([]);
  });

  it("leaves the tab where it was when the pointer stream is cancelled", () => {
    travel({ x: 500, y: 900 });
    act(() => {
      window.dispatchEvent(
        new PointerEvent("pointercancel", { clientX: 500, clientY: 900, pointerId: 1 }),
      );
    });
    expect(dropped).toEqual([]);
    expect(dragging()).toBe(false);
  });

  it.each([
    ["a window blur", () => window.dispatchEvent(new Event("blur"))],
    ["the page being frozen", () => document.dispatchEvent(new Event("freeze"))],
    [
      "the document being hidden",
      () => {
        Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
        document.dispatchEvent(new Event("visibilitychange"));
      },
    ],
  ])("ends the gesture on %s", (_label, happen) => {
    travel({ x: 500, y: 900 });
    expect(dragging()).toBe(true);

    act(() => happen());

    expect(dropped).toEqual([]);
    expect(preview()).toBeNull();
    expect(dragging()).toBe(false);
    expect(chipRelease).toHaveBeenCalledWith(1);
    // The pointerup that may follow is not a drop either.
    release({ x: 500, y: 900 });
    expect(dropped).toEqual([]);
  });

  it("ends the gesture when the dragged tab has gone", () => {
    travel({ x: 500, y: 900 });
    openTabs = new Set();
    move({ x: 500, y: 700 });

    expect(dropped).toEqual([]);
    expect(preview()).toBeNull();
    expect(dragging()).toBe(false);
    release({ x: 500, y: 700 });
    expect(dropped).toEqual([]);
  });

  it("does not report a released capture the chip never held", () => {
    chipHolds.mockReturnValue(false);
    travel({ x: 500, y: 900 });
    release({ x: 500, y: 900 });
    expect(dropped).toEqual([[TAB, "bottom"]]);
    expect(chipRelease).not.toHaveBeenCalled();
  });

  it("ignores a pointer it never pressed with", () => {
    press();
    act(() => {
      window.dispatchEvent(
        new PointerEvent("pointerup", { clientX: 500, clientY: 900, pointerId: 7 }),
      );
    });
    expect(dropped).toEqual([]);
  });
});

describe("what a pointer move costs the surface above the drag", () => {
  it("re-renders nothing while the zone does not change", () => {
    travel({ x: 500, y: 700 });
    const layerCommits = commits;
    const rendersBefore = parentRenders;

    for (const y of [720, 760, 800, 840]) move({ x: 500, y });

    expect(commits).toBe(layerCommits);
    expect(parentRenders).toBe(rendersBefore);
    release({ x: 500, y: 840 });
  });

  it("re-renders once when the zone changes", () => {
    travel({ x: 500, y: 700 });
    const layerCommits = commits;

    move({ x: 500, y: 300 });

    expect(commits).toBe(layerCommits + 1);
    release({ x: 500, y: 300 });
  });
});
