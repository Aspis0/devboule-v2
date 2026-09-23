// Why this file exists: the "+" menu and the provider picker used to open
// inside the strip's overflow, where the centre panel clips them and the
// right panel's resize handle covers their entries (measured: elementFromPoint
// at both entries returned .workspace-resize-handle). They render through
// this portal — document.body, positioned fixed from the anchor's
// getBoundingClientRect — so no panel's overflow or later sibling can touch
// them. The placement arithmetic is pure and unit-tested in
// popoverPlace.test.ts; happy-dom has no layout to prove the wiring against.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  type HTMLAttributes,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";

/** Viewport margin the popover keeps on every edge when it is clamped. */
export const POPOVER_MARGIN = 8;
/** The gap the popover keeps off its anchor, on whichever side it opens. */
const ANCHOR_GAP = 6;

export interface PopoverPlacement {
  left: number;
  top: number;
  /** The width the popover is given: the viewport minus both margins. */
  maxWidth: number;
  /** The height it is given on the side it opened, so the rest is an inside scroll. */
  maxHeight: number;
}

/**
 * Where a popover of `popover` size goes for an `anchor` box in a `viewport`.
 *
 * Horizontal: right of the anchor when it fits — left edge on the anchor's
 * left edge, as it always opened; when it does not fit, the popover's RIGHT
 * edge aligns with the anchor's right edge; either way clamped inside the
 * viewport, and `maxWidth` caps the popover so "wider than the viewport"
 * ends at the margin instead of off-screen.
 *
 * Vertical: it opens BELOW the anchor with `maxHeight` = the space left
 * below minus the margin, and flips ABOVE only when the space above is
 * larger — in both directions the popover's visible edge keeps its gap off
 * the anchor: it never covers it.
 */
export function placePopover(
  anchor: { left: number; right: number; top: number; bottom: number },
  popover: { width: number; height: number },
  viewport: { width: number; height: number },
  margin: number = POPOVER_MARGIN,
): PopoverPlacement {
  const maxWidth = Math.max(0, viewport.width - margin * 2);
  const width = Math.min(popover.width, maxWidth);
  const fitsRight = anchor.right + width <= viewport.width - margin;
  const wanted = fitsRight ? anchor.left : anchor.right - width;
  const maxLeft = Math.max(margin, viewport.width - margin - width);
  const left = Math.min(Math.max(wanted, margin), maxLeft);

  const spaceBelow = viewport.height - anchor.bottom - margin;
  const spaceAbove = anchor.top - margin;
  const below = spaceAbove <= spaceBelow;
  const maxHeight = Math.max(
    0,
    below
      ? viewport.height - (anchor.bottom + ANCHOR_GAP) - margin
      : anchor.top - ANCHOR_GAP - margin,
  );
  const height = Math.min(popover.height, maxHeight);
  const top = below ? anchor.bottom + ANCHOR_GAP : anchor.top - ANCHOR_GAP - height;
  return { left, top, maxWidth, maxHeight };
}

interface AnchoredPopoverProps extends HTMLAttributes<HTMLDivElement> {
  /** The element the popover hangs from; its rectangle sets the position. */
  anchorRef: RefObject<HTMLElement | null>;
  /** The caller's outside-click root: a press inside the portal counts as inside. */
  containerRef?: RefObject<HTMLDivElement | null>;
  /** Close the popover: an ancestor of the anchor scrolled, or the anchor left the document. */
  onDismiss?: () => void;
  children: ReactNode;
}

/**
 * One portal for both strip popovers. Placement is measured with the final
 * position and width constraints already applied (a block child of body
 * fills the body — measuring it first is measuring the wrong width), runs
 * for its inputs only — the anchor at open and the popover's own size via a
 * ResizeObserver, never after every parent commit — and the popover closes
 * through `onDismiss` when its anchor's world moves: a scroll is dismissed
 * only when the anchor's rectangle no longer matches the one placement used
 * (focusing an entry scrolls the document without moving the anchor — that
 * scroll is ignored, whatever its target), or when the anchor left the
 * document.
 */
export function AnchoredPopover({
  anchorRef,
  containerRef,
  onDismiss,
  children,
  ...divProps
}: AnchoredPopoverProps) {
  const rootRef = useRef<HTMLDivElement>(null);
  const anchorRectRef = useRef<{ left: number; top: number; right: number; bottom: number } | null>(
    null,
  );
  const assignRefs = (node: HTMLDivElement | null) => {
    rootRef.current = node;
    if (containerRef !== undefined) containerRef.current = node;
  };

  const place = useCallback(() => {
    const root = rootRef.current;
    const anchor = anchorRef.current;
    if (root === null || anchor === null) return;
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    // Constraints first: position: fixed makes the box shrink-to-fit and
    // max-width caps it, so the rectangle measured next is the real one.
    root.style.position = "fixed";
    root.style.maxWidth = `${Math.max(0, viewport.width - POPOVER_MARGIN * 2)}px`;
    root.style.overflow = "auto";
    const anchorBox = anchor.getBoundingClientRect();
    const rootBox = root.getBoundingClientRect();
    anchorRectRef.current = {
      left: anchorBox.left,
      top: anchorBox.top,
      right: anchorBox.right,
      bottom: anchorBox.bottom,
    };
    const placed = placePopover(anchorBox, rootBox, viewport);
    root.style.left = `${placed.left}px`;
    root.style.top = `${placed.top}px`;
    root.style.maxHeight = `${placed.maxHeight}px`;
    // Above the resize handles: the portal is a body child, and the explicit
    // number keeps it above any stacking context the app root may create.
    root.style.zIndex = "100";
    root.style.minWidth = "220px";
  }, [anchorRef]);

  useLayoutEffect(() => {
    place();
    const root = rootRef.current;
    if (root === null || typeof ResizeObserver === "undefined") return;
    // The picker and its consent card swap size while open; nothing else
    // changes these inputs (an anchor move closes the popover instead).
    const observer = new ResizeObserver(place);
    observer.observe(root);
    return () => observer.disconnect();
  }, [place]);

  useEffect(() => {
    if (onDismiss === undefined) return;
    // Dismiss a scroll only if the ANCHOR moved: compare its rectangle with
    // the one placement used. Focusing an entry inside the portal scrolls
    // the document (#document) without moving the anchor — that scroll must
    // not close the popover as it opens, whatever its target.
    const onScroll = () => {
      const anchor = anchorRef.current;
      const placed = anchorRectRef.current;
      if (anchor === null || placed === null) return;
      const now = anchor.getBoundingClientRect();
      const moved =
        now.left !== placed.left ||
        now.top !== placed.top ||
        now.right !== placed.right ||
        now.bottom !== placed.bottom;
      if (moved) onDismiss();
    };
    window.addEventListener("scroll", onScroll, true);
    const anchor = anchorRef.current;
    const observer = new MutationObserver(() => {
      if (anchor !== null && !anchor.isConnected) onDismiss();
    });
    observer.observe(document.body, { childList: true, subtree: true });
    return () => {
      window.removeEventListener("scroll", onScroll, true);
      observer.disconnect();
    };
  }, [anchorRef, onDismiss]);

  return createPortal(
    <div ref={assignRefs} {...divProps}>
      {children}
    </div>,
    document.body,
  );
}
