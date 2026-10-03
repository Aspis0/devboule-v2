// Where one browser page is on screen, and when the controller needs to hear
// about it. The page is a child webview the pane measures: it moves with the
// pane, the window and the divider, and it must get out of the way entirely
// when an overlay covers it.
//
// Three things can want to move the page and all three can fire in a burst —
// a drag sends a resize per frame, a window move sends a scroll per element —
// so they all end here, and here sends at most one rectangle per animation
// frame, the newest one, and never the same rectangle twice. Parking is the
// same decision with a different answer: the page keeps running either way,
// because a hidden WebView2 throttles it to about 1 Hz.

import { browserPark, browserPresent, browserRectOf } from "./browserController";
import { overlaysCover, subscribeOverlays } from "./browserOverlays";
import type { LogicalRect } from "../../types/ipc";

/** What the pane hands back to drive the one decision it cannot make itself:
 * the page exists, and the page has gone. */
export interface BrowserPagePlacement {
  /** The controller owns this page now, so it can be placed. */
  show: () => void;
  /** This pane is gone. The page keeps running; it is parked, not disposed. */
  dispose: () => void;
}

interface Placement {
  id: string;
  area: HTMLElement;
  /** The rectangle the controller last heard, or null while it is parked: a
   * parked page has to be told its rectangle again even if nothing moved. */
  sent: LogicalRect | null;
  parked: boolean;
  /** The newest rectangle measured, waiting for the frame that will send it. */
  pending: LogicalRect | null;
  frame: number | null;
  /** Whether the controller owns this page yet. Nothing is placed before the
   * create has answered: there is nothing to place. */
  owned: boolean;
  /** Whether an open overlay covers the page right now. */
  covered: boolean;
  stop: Array<() => void>;
}

/** The same rectangle, field by field: each measure builds a fresh object, so
 * the controller is only told again when the numbers differ. */
function sameRect(sent: LogicalRect | null, measured: LogicalRect): boolean {
  return (
    sent !== null &&
    sent.x === measured.x &&
    sent.y === measured.y &&
    sent.width === measured.width &&
    sent.height === measured.height
  );
}

function measure(placement: Placement): void {
  if (!placement.owned) return;
  placement.pending = browserRectOf(placement.area.getBoundingClientRect());
  if (placement.frame !== null) return;
  placement.frame = requestAnimationFrame(() => flush(placement));
}

function flush(placement: Placement): void {
  placement.frame = null;
  const rect = placement.pending;
  placement.pending = null;
  if (rect === null) return;
  if (placement.covered) {
    if (placement.parked) return;
    placement.parked = true;
    placement.sent = null;
    void browserPark(placement.id).catch(() => undefined);
    return;
  }
  if (sameRect(placement.sent, rect)) return;
  placement.parked = false;
  placement.sent = rect;
  void browserPresent(placement.id, rect).catch(() => undefined);
}

/**
 * Follow one page's area for as long as the pane owns it. The caller says
 * when the page exists, because the create is what knows.
 */
export function followBrowserPage(id: string, area: HTMLElement): BrowserPagePlacement {
  const placement: Placement = {
    id,
    area,
    sent: null,
    parked: false,
    pending: null,
    frame: null,
    owned: false,
    covered: false,
    stop: [],
  };
  const onLayout = (): void => measure(placement);

  const observer = new ResizeObserver(onLayout);
  observer.observe(area);
  placement.stop.push(() => observer.disconnect());
  window.addEventListener("resize", onLayout);
  window.addEventListener("scroll", onLayout, { capture: true, passive: true });
  placement.stop.push(() => {
    window.removeEventListener("resize", onLayout);
    window.removeEventListener("scroll", onLayout, { capture: true });
  });

  placement.stop.push(
    subscribeOverlays(() => {
      // Re-read on every overlay change: a menu that opened over the page, a
      // menu that closed, and a dialog that moved are all the same answer
      // asked again with new numbers.
      const covered = overlaysCover(area.getBoundingClientRect());
      if (covered === placement.covered) return;
      placement.covered = covered;
      onLayout();
    }),
  );

  return {
    show: () => {
      placement.owned = true;
      measure(placement);
    },
    dispose: () => {
      placement.owned = false;
      if (placement.frame !== null) cancelAnimationFrame(placement.frame);
      placement.frame = null;
      placement.pending = null;
      for (const stop of placement.stop.splice(0)) stop();
      // Park, never dispose: a tab that is merely no longer in front keeps its
      // page alive and running, and a park for a tab that is already gone has
      // nothing to park.
      void browserPark(id).catch(() => undefined);
    },
  };
}
