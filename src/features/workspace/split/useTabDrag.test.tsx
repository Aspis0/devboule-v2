// @vitest-environment happy-dom

// The gesture, with no workspace around it: a press that travels is a drag, a
// press that does not is a click, the zone follows the pointer, and Escape ends
// a drag without acting. The preview it asks for is the same object the
// workspace renders.

import { act, useEffect } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useTabDrag, DRAG_SLOP_PX, type TabDrag } from "./useTabDrag";
import { SplitDropPreview } from "./SplitDropPreview";
import type { DropZone } from "./tabDropZones";

/** The centre a drop is read against, and the strip above it. */
const CENTER = { left: 0, top: 100, width: 1000, height: 800, right: 1000, bottom: 900 };
const STRIP = { left: 0, top: 40, width: 1000, height: 40, right: 1000, bottom: 80 };

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;
let drag: TabDrag | null = null;
let dropped: Array<[string, DropZone | "strip"]> = [];
let start:
  | ((tabId: string, event: { clientX: number; clientY: number; pointerId: number }) => void)
  | null = null;

function box(rect: { left: number; top: number; width: number; height: number }): DOMRect {
  return {
    ...rect,
    right: rect.left + rect.width,
    bottom: rect.top + rect.height,
    toJSON: () => ({}),
  } as DOMRect;
}

function Harness() {
  const gesture = useTabDrag({
    boxes: () => ({
      area: { getBoundingClientRect: () => box(CENTER) } as HTMLElement,
      strip: { getBoundingClientRect: () => box(STRIP) } as HTMLElement,
    }),
    onDrop: (tabId, zone) => dropped.push([tabId, zone]),
  });
  // Reported after the render, so the case reads the gesture rather than the
  // component writing to the module while it draws.
  useEffect(() => {
    drag = gesture.drag;
    start = gesture.startDrag;
  });
  return <SplitDropPreview zone={gesture.drag?.zone ?? null} />;
}

/** A press, then a travel to `to`, which is where the pointer ends up. */
function travel(to: { x: number; y: number }, from = { x: 500, y: 60 }): void {
  act(() => {
    start?.("tab-1", { clientX: from.x, clientY: from.y, pointerId: 1 });
  });
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

beforeEach(() => {
  drag = null;
  start = null;
  dropped = [];
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  act(() => root.render(<Harness />));
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe("a chip press that travels", () => {
  it("is a drag, and the preview follows the pointer", () => {
    travel({ x: 500, y: 200 }); // the centre's top band
    expect(drag?.tabId).toBe("tab-1");
    expect(drag?.zone).toBe("top");
    expect(preview()?.getAttribute("data-zone")).toBe("top");

    act(() => {
      window.dispatchEvent(
        new PointerEvent("pointermove", { clientX: 500, clientY: 900, pointerId: 1 }),
      );
    });
    expect(drag?.zone).toBe("bottom");
    // Let go of it: a case that leaves a drag open would hand its gesture to the
    // next one.
    release({ x: 500, y: 900 });
  });

  it("is nothing at all until it passes the slop", () => {
    act(() => {
      start?.("tab-1", { clientX: 500, clientY: 60, pointerId: 1 });
    });
    act(() => {
      window.dispatchEvent(
        new PointerEvent("pointermove", {
          clientX: 500 + DRAG_SLOP_PX - 1,
          clientY: 60,
          pointerId: 1,
        }),
      );
    });
    expect(drag).toBeNull();
    expect(preview()).toBeNull();
    expect(document.body.classList.contains("workspace-is-dragging-tab")).toBe(false);

    act(() => {
      window.dispatchEvent(
        new PointerEvent("pointermove", { clientX: 500 + DRAG_SLOP_PX, clientY: 60, pointerId: 1 }),
      );
    });
    expect(drag).not.toBeNull();
    // The drag this one began is let go here, so the gesture does not outlive
    // the case that started it.
    release({ x: 500 + DRAG_SLOP_PX, y: 60 });
    dropped.length = 0;
  });

  it("selects nothing while it is held", () => {
    travel({ x: 500, y: 200 });
    expect(document.body.classList.contains("workspace-is-dragging-tab")).toBe(true);
    release({ x: 500, y: 200 });
    expect(document.body.classList.contains("workspace-is-dragging-tab")).toBe(false);
    dropped.length = 0;
  });

  it("says strip when the pointer is back over the tab row", () => {
    // Pressed over the centre and carried up onto the tab row itself.
    travel({ x: 500, y: 60 }, { x: 500, y: 300 });
    expect(drag?.zone).toBe("strip");
    // The strip draws no preview of its own: the tab is over its own row.
    expect(preview()).toBeNull();
    release({ x: 500, y: 60 });
    dropped.length = 0;
  });

  it("has no preview over neither box", () => {
    travel({ x: 1500, y: 600 });
    expect(drag?.zone).toBeNull();
    expect(preview()).toBeNull();
    release({ x: 1500, y: 600 });
  });
});

describe("letting go", () => {
  it("reports the zone under the pointer", () => {
    travel({ x: 500, y: 900 });
    release({ x: 500, y: 900 });
    expect(dropped).toEqual([["tab-1", "bottom"]]);
    expect(drag).toBeNull();
  });

  it("does nothing at all for a press that never moved", () => {
    act(() => {
      start?.("tab-1", { clientX: 500, clientY: 60, pointerId: 1 });
    });
    release({ x: 500, y: 60 });
    expect(dropped).toEqual([]);
  });

  it("acts on nothing when the drop lands outside both boxes", () => {
    travel({ x: 1500, y: 600 });
    release({ x: 1500, y: 600 });
    expect(dropped).toEqual([]);
  });

  it("leaves the tab where it was on Escape", () => {
    travel({ x: 500, y: 900 });
    escape();
    expect(drag).toBeNull();
    expect(preview()).toBeNull();
    expect(dropped).toEqual([]);
    // The pointerup that follows the escape is not a drop either.
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
    expect(drag).toBeNull();
  });

  it("ignores a pointer it never pressed with", () => {
    act(() => {
      start?.("tab-1", { clientX: 500, clientY: 60, pointerId: 1 });
    });
    act(() => {
      window.dispatchEvent(
        new PointerEvent("pointerup", { clientX: 500, clientY: 900, pointerId: 7 }),
      );
    });
    expect(dropped).toEqual([]);
  });
});
