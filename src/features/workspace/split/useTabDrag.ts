// Why: a drag of a strip chip has to start as a click and become a drag later.
// The pointer is captured only once it has moved past a slop, so the click that
// selects a tab stays a click, and the tab row's own selection never fights the
// drag for the same gesture.
//
// Nothing here decides what a drop means: the gesture reports where the pointer
// is, and the caller resolves that against the panes. Escape ends a drag without
// acting, because a person who pressed it wants their tab back where it was.

import { useCallback, useEffect, useRef, useState } from "react";
import { resolveDropZone, type DropZone } from "./tabDropZones";

/** How far the pointer has to travel before a press is a drag rather than a
 * click. Four pixels is past a press, short enough that a deliberate drag never
 * feels like it starts late. */
export const DRAG_SLOP_PX = 4;

/** What the drag is showing right now. `zone` is null until the pointer has left
 * the chip, so nothing is previewed over the strip while the press is still a
 * click. */
export interface TabDrag {
  tabId: string;
  zone: DropZone | "strip" | null;
}

interface Press {
  tabId: string;
  pointerId: number;
  /** Where the press began, which is what the slop is measured from. */
  startX: number;
  startY: number;
  /** The pointer's latest position, which is what a zone is read from. */
  x: number;
  y: number;
}

/** The boxes the pointer is read against. Both are read live: the strip and the
 * centre move as the window does. */
export interface TabDragBoxes {
  /** The workspace centre: the pane a drop is resolved against. */
  area: HTMLElement | null;
  /** The strip: a drop there is a tab going back to its row. */
  strip: HTMLElement | null;
}

export interface UseTabDragOptions {
  boxes: () => TabDragBoxes;
  /** What a drop does, decided by the caller against the live panes. */
  onDrop: (tabId: string, zone: DropZone | "strip") => void;
}

function zoneAt(boxes: TabDragBoxes, x: number, y: number): DropZone | "strip" | null {
  const strip = boxes.strip?.getBoundingClientRect();
  if (strip !== undefined && inside(strip, x, y)) return "strip";
  const area = boxes.area?.getBoundingClientRect();
  if (area === undefined || !inside(area, x, y)) return null;
  return resolveDropZone({
    width: area.width,
    height: area.height,
    x: x - area.left,
    y: y - area.top,
  });
}

function inside(rect: DOMRect, x: number, y: number): boolean {
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

export function useTabDrag({ boxes, onDrop }: UseTabDragOptions): {
  drag: TabDrag | null;
  /** Called from the strip's own pointerdown, with the chip under the pointer. */
  startDrag: (
    tabId: string,
    event: { clientX: number; clientY: number; pointerId: number },
  ) => void;
} {
  const [drag, setDrag] = useState<TabDrag | null>(null);
  const pressRef = useRef<Press | null>(null);
  const draggingRef = useRef(false);
  // The latest caller values, read through refs and written after the render:
  // the listeners below are installed once, and re-installing them on every
  // render would take the drag's body class off while the drag is still on.
  const onDropRef = useRef(onDrop);
  const boxesRef = useRef(boxes);
  useEffect(() => {
    onDropRef.current = onDrop;
    boxesRef.current = boxes;
  });
  const boxesNow = useCallback((): TabDragBoxes => boxesRef.current(), []);

  const finish = useCallback(
    (act: ((tabId: string, zone: DropZone | "strip") => void) | null) => {
      const press = pressRef.current;
      pressRef.current = null;
      if (!draggingRef.current) return;
      draggingRef.current = false;
      document.body.classList.remove("workspace-is-dragging-tab");
      const zone = press === null ? null : zoneAt(boxesNow(), press.x, press.y);
      setDrag(null);
      if (zone !== null && act !== null && press !== null) act(press.tabId, zone);
    },
    [boxesNow],
  );

  useEffect(() => {
    const move = (event: PointerEvent): void => {
      const press = pressRef.current;
      if (press === null) return;
      if (event.pointerId !== press.pointerId) return;
      if (!draggingRef.current) {
        const travelled = Math.hypot(event.clientX - press.startX, event.clientY - press.startY);
        if (travelled < DRAG_SLOP_PX) return;
      }
      press.x = event.clientX;
      press.y = event.clientY;
      if (!draggingRef.current) {
        // Past the slop, and only now: the capture and the no-selection class
        // belong to a drag, never to a press.
        draggingRef.current = true;
        document.body.classList.add("workspace-is-dragging-tab");
        (event.target as Element | null)?.setPointerCapture?.(press.pointerId);
      }
      const zone = zoneAt(boxesNow(), event.clientX, event.clientY);
      setDrag({ tabId: press.tabId, zone });
    };
    const up = (event: PointerEvent): void => {
      if (pressRef.current === null || event.pointerId !== pressRef.current.pointerId) return;
      const press = pressRef.current;
      press.x = event.clientX;
      press.y = event.clientY;
      finish((tabId, zone) => onDropRef.current(tabId, zone));
    };
    const cancel = (event: PointerEvent): void => {
      if (pressRef.current === null || event.pointerId !== pressRef.current.pointerId) return;
      // A stream the app never finished: the tab is where it was.
      finish(null);
    };
    const escape = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || pressRef.current === null) return;
      event.preventDefault();
      finish(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", escape);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", escape);
      // A drag cannot outlive the surface it is dragging over: the press, the
      // flag and the body class all go with the listeners.
      pressRef.current = null;
      draggingRef.current = false;
      document.body.classList.remove("workspace-is-dragging-tab");
    };
  }, [boxesNow, finish]);

  const startDrag = useCallback(
    (tabId: string, event: { clientX: number; clientY: number; pointerId: number }) => {
      pressRef.current = {
        tabId,
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        x: event.clientX,
        y: event.clientY,
      };
    },
    [],
  );

  return { drag, startDrag };
}
