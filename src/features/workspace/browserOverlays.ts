// Every open overlay that could sit over a browser page, and whether one of
// them covers a given rectangle. The page is a child webview — a native
// window above the app's own — so DOM z-index cannot put a menu or a dialog
// over it: the only thing the app can do is move the page out of the way
// while an overlay is on screen.
//
// The registry is the single place that answer comes from, because "an
// overlay is open" is not a fact any one primitive owns: a context menu and a
// confirm dialog are different components with no common parent, and a page
// under the strip is covered by whichever of them is open.

/** A registered overlay as a live box: its rect is read when it is asked for,
 * not when it was registered, because a popover follows its anchor. */
type OverlayBox = () => DOMRect;

const open = new Set<OverlayBox>();
const listeners = new Set<() => void>();
let revision = 0;

/** Tell every watcher that the set of open overlays, or one of their boxes,
 * has changed. */
function publish(): void {
  revision += 1;
  for (const listener of [...listeners]) listener();
}

/** Do two boxes share any of the plane? Touching edges do not count: a menu
 * that ends exactly where the page starts has not covered it. */
export function overlaysIntersect(cover: DOMRect, covered: DOMRect): boolean {
  return (
    cover.left < covered.right &&
    covered.left < cover.right &&
    cover.top < covered.bottom &&
    covered.top < cover.bottom
  );
}

/** Whether any open overlay covers `page`. */
export function overlaysCover(page: DOMRect): boolean {
  for (const box of open) {
    if (overlaysIntersect(box(), page)) return true;
  }
  return false;
}

/**
 * Register an overlay for as long as it is on screen. The caller registers
 * while it is open, which is what makes an overlay's absence the default:
 * nothing is ever parked for an overlay that is not there.
 */
export function registerOverlay(element: HTMLElement | null): () => void {
  if (element === null) return () => undefined;
  const box: OverlayBox = () => element.getBoundingClientRect();
  open.add(box);
  // A menu that moves with its anchor, or a dialog that follows a resize, can
  // start or stop covering the page without ever reopening.
  const observer = new ResizeObserver(publish);
  observer.observe(element);
  publish();
  return () => {
    if (!open.delete(box)) return;
    observer.disconnect();
    publish();
  };
}

/** Changes with every open, close and move, and with nothing else. */
export function overlaysRevision(): number {
  return revision;
}

export function subscribeOverlays(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function resetBrowserOverlaysForTests(): void {
  open.clear();
  listeners.clear();
  revision = 0;
}
