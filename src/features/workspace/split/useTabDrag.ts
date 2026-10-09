// Why: a drag of a strip chip has to start as a click and become a drag later.
// The pointer is captured only once it has moved past a slop, on the element
// that started the gesture, and released on every way out of it — so the click
// that selects a tab stays a click, and a control the pointer happens to be
// over is never the one left holding the pointer.
//
// Nothing here decides what a drop means: the gesture reports where the pointer
// is, and the caller resolves that against the panes. Escape, a lost pointer,
// a hidden window and a dragged tab that has gone all end the gesture the same
// way — without acting, and with the page presented again.

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
  mark: TabInsertionMark | null;
}

/** Where a tab dragged over the tab row would land: the viewport x of the gap,
 * and the row's box for the line that marks it. */
export interface TabInsertionMark {
  x: number;
  top: number;
  height: number;
}

interface Press {
  tabId: string;
  pointerId: number;
  /** The element that owns the gesture: capture and release belong to it. */
  owner: Element;
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

/** Where the pointer was let go, in viewport pixels: a strip drop reads the
 * place on the row from it. */
export interface TabDropPoint {
  x: number;
  y: number;
}

export interface UseTabDragOptions {
  boxes: () => TabDragBoxes;
  /** What a drop does, decided by the caller against the live panes. */
  onDrop: (tabId: string, zone: DropZone | "strip", point: TabDropPoint) => void;
  /** Where a tab dropped on the tab row would land, or null when nothing is shown. */
  markAt: (tabId: string, point: TabDropPoint) => TabInsertionMark | null;
  /** Whether the tab is still open. A tab an agent closes mid-gesture ends it. */
  hasTab: (tabId: string) => boolean;
}

function sameMark(a: TabInsertionMark | null, b: TabInsertionMark | null): boolean {
  if (a === null || b === null) return a === b;
  return a.x === b.x && a.top === b.top && a.height === b.height;
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

export function useTabDrag({ boxes, onDrop, markAt, hasTab }: UseTabDragOptions): {
  /** The zone the preview answers, and nothing else: a pointer that moves
   * inside one zone must not re-render the surface that owns this hook. */
  zone: DropZone | "strip" | null;
  /** The tab under the pointer, while a gesture is on. */
  tabId: string | null;
  /** The gap on the tab row the drop would land in, for the line that marks it. */
  mark: TabInsertionMark | null;
  /** Called from the strip's own pointerdown, with the chip that was pressed. */
  startDrag: (
    tabId: string,
    owner: Element,
    event: { clientX: number; clientY: number; pointerId: number },
  ) => void;
} {
  const [drag, setDrag] = useState<TabDrag | null>(null);
  const pressRef = useRef<Press | null>(null);
  const draggingRef = useRef(false);
  const optionsRef = useRef({ boxes, onDrop, markAt, hasTab });
  // The latest caller values, read through a ref and written after the render:
  // the listeners below are installed once, and re-installing them on every
  // render would take the drag's body class off while the drag is still on.
  useEffect(() => {
    optionsRef.current = { boxes, onDrop, markAt, hasTab };
  });
  const now = useCallback(() => optionsRef.current, []);

  const end = useCallback(
    (zone: DropZone | "strip" | null) => {
      const press = pressRef.current;
      pressRef.current = null;
      if (!draggingRef.current) return;
      draggingRef.current = false;
      document.body.classList.remove("workspace-is-dragging-tab");
      // Every way out hands the pointer back, including the one where the
      // pointer stream itself was lost.
      releaseCapture(press);
      setDrag(null);
      if (zone !== null && press !== null) {
        now().onDrop(press.tabId, zone, { x: press.x, y: press.y });
      }
    },
    [now],
  );

  useEffect(() => {
    const move = (event: PointerEvent): void => {
      const press = pressRef.current;
      if (press === null || event.pointerId !== press.pointerId) return;
      // The tab this gesture is about has gone: a drop would name a tab the
      // workspace no longer has.
      if (!now().hasTab(press.tabId)) {
        end(null);
        return;
      }
      if (!draggingRef.current) {
        const travelled = Math.hypot(event.clientX - press.startX, event.clientY - press.startY);
        if (travelled < DRAG_SLOP_PX) return;
        draggingRef.current = true;
        document.body.classList.add("workspace-is-dragging-tab");
        capture(press.owner, press.pointerId);
      }
      press.x = event.clientX;
      press.y = event.clientY;
      // The zone and the gap are state, and only when one changes: a pointer
      // moving inside one destination must not re-render the surface.
      const zone = zoneAt(now().boxes(), event.clientX, event.clientY);
      const mark =
        zone === "strip" ? now().markAt(press.tabId, { x: event.clientX, y: event.clientY }) : null;
      setDrag((current) =>
        current?.zone === zone && sameMark(current.mark, mark)
          ? current
          : { tabId: press.tabId, zone, mark },
      );
    };
    const up = (event: PointerEvent): void => {
      const press = pressRef.current;
      if (press === null || event.pointerId !== press.pointerId) return;
      if (!now().hasTab(press.tabId)) {
        end(null);
        return;
      }
      press.x = event.clientX;
      press.y = event.clientY;
      end(zoneAt(now().boxes(), event.clientX, event.clientY));
    };
    const cancel = (event: PointerEvent): void => {
      const press = pressRef.current;
      if (press === null || event.pointerId !== press.pointerId) return;
      end(null);
    };
    const escape = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || pressRef.current === null) return;
      event.preventDefault();
      end(null);
    };
    // The pointer can leave the app without an event ever arriving: a window
    // blur, a hidden tab, or a capture the surface took back. Each of those ends
    // the gesture where it stands.
    // A frozen page is the same thing a hidden one is, and it is what a window
    // the system has taken over looks like from here.
    const hidden = (): void => end(null);
    const captured = (event: PointerEvent): void => {
      const press = pressRef.current;
      if (press === null) return;
      if (event.pointerId !== press.pointerId) return;
      if (event.target !== press.owner) return;
      end(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", escape);
    window.addEventListener("blur", hidden);
    document.addEventListener("visibilitychange", hidden);
    document.addEventListener("freeze", hidden);
    window.addEventListener("lostpointercapture", captured);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", escape);
      window.removeEventListener("blur", hidden);
      document.removeEventListener("visibilitychange", hidden);
      document.removeEventListener("freeze", hidden);
      window.removeEventListener("lostpointercapture", captured);
      // The layer can go while a drag is on, and a chip that kept the pointer
      // would not give it back to the controls the pointer is over next.
      releaseCapture(pressRef.current);
      pressRef.current = null;
      draggingRef.current = false;
      document.body.classList.remove("workspace-is-dragging-tab");
    };
  }, [end, now]);

  const startDrag = useCallback(
    (
      tabId: string,
      owner: Element,
      event: { clientX: number; clientY: number; pointerId: number },
    ) => {
      pressRef.current = {
        tabId,
        owner,
        pointerId: event.pointerId,
        startX: event.clientX,
        startY: event.clientY,
        x: event.clientX,
        y: event.clientY,
      };
    },
    [],
  );

  return {
    zone: drag?.zone ?? null,
    tabId: drag?.tabId ?? null,
    mark: drag?.mark ?? null,
    startDrag,
  };
}

/** Capture is best-effort: a surface that refuses it still gets the gesture. */
function capture(owner: Element, pointerId: number): void {
  (owner as Element & { setPointerCapture?: (id: number) => void }).setPointerCapture?.(pointerId);
}

function releaseCapture(press: Press | null): void {
  if (press === null) return;
  const owner = press.owner as Element & {
    releasePointerCapture?: (id: number) => void;
    hasPointerCapture?: (id: number) => boolean;
  };
  if (owner.hasPointerCapture?.(press.pointerId) !== true) return;
  try {
    owner.releasePointerCapture?.(press.pointerId);
  } catch {
    // A capture the surface has already taken back is not a failure to release.
  }
}
