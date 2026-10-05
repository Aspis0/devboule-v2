// The workspace centre, split: the workspace's own pane on top, one tool tab
// in a pane below, and the divider between them. Nothing here decides what a
// tab is — the caller resolves the pane's tab — and nothing here persists: a
// workspace with no split renders no wrapper at all, so the DOM of an unsplit
// workspace is the one it had before the split existed.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
} from "react";
import {
  clampSplitSizeForArea,
  splitBoundsFor,
  splitSizeFromKey,
  splitSizeFromPointer,
} from "./splitGeometry";
import type { SplitPaneRecord } from "./splitPaneStorage";
import "./SplitPane.css";

export interface SplitPaneProps {
  /** The workspace's split, or null while the workspace is one pane. */
  split: SplitPaneRecord | null;
  /** The divider's answer, written once at the end of a drag. */
  onResize: (size: number) => void;
  onMerge: () => void;
  /** What the pane below is showing, named in its own header. */
  lowerLabel: string;
  /** The tab in the pane below. */
  lower: ReactNode;
  /** The workspace's own pane: whatever the centre has always shown. */
  children: ReactNode;
}

export function SplitPane({ split, ...rest }: SplitPaneProps) {
  return split === null ? <>{rest.children}</> : <SplitArea split={split} {...rest} />;
}

/** The divider's geometry while a pointer is on it. The live size is state so
 * the panes follow the pointer, and the store takes it once at the end: a drag
 * is a frame per pixel, and a storage write per frame is nobody's idea of a
 * divider. */
interface Drag {
  top: number;
  height: number;
  size: number;
}

function SplitArea({
  split,
  onResize,
  onMerge,
  lowerLabel,
  lower,
  children,
}: Omit<SplitPaneProps, "split"> & { split: SplitPaneRecord }) {
  const areaRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<Drag | null>(null);
  const [dragSize, setDragSize] = useState<number | null>(null);
  const [dragging, setDragging] = useState(false);
  /** The split area's own height, which is what the two pixel floors are a
   * share OF. Measured, not assumed: the window is resizable and the floors are
   * pixels. Zero until the observer reports, which leaves the fraction bounds. */
  const [areaHeight, setAreaHeight] = useState(0);
  const bounds = splitBoundsFor(areaHeight);
  const size = clampSplitSizeForArea(dragSize ?? split.size, areaHeight);

  useEffect(() => {
    const area = areaRef.current;
    if (area === null) return;
    const measure = (): void => setAreaHeight(area.getBoundingClientRect().height);
    measure();
    // The area grows with the window and with the side panels, and the observer
    // only reports the box it watches; the window event is what a side panel's
    // own resize ends in, and it costs one measurement.
    const observer = new ResizeObserver(measure);
    observer.observe(area);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  }, []);

  const startDrag = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const area = areaRef.current;
      if (area === null) return;
      event.preventDefault();
      const box = area.getBoundingClientRect();
      dragRef.current = { top: box.top, height: box.height, size };
      setDragging(true);
      document.body.classList.add("workspace-split-is-dragging");
    },
    [size],
  );

  useEffect(() => {
    if (!dragging) return;
    const move = (event: globalThis.PointerEvent): void => {
      const drag = dragRef.current;
      if (drag === null) return;
      const next = splitSizeFromPointer(event.clientY, drag.top, drag.height);
      drag.size = next;
      setDragSize(next);
    };
    const end = (): void => {
      const drag = dragRef.current;
      dragRef.current = null;
      setDragging(false);
      setDragSize(null);
      document.body.classList.remove("workspace-split-is-dragging");
      // A press and a release with no move in between is a click on the
      // divider, and it has nothing to say about the size.
      if (drag !== null && drag.size !== split.size) onResize(drag.size);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
    window.addEventListener("pointercancel", end);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
      window.removeEventListener("pointercancel", end);
      document.body.classList.remove("workspace-split-is-dragging");
    };
  }, [dragging, onResize, split.size]);

  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      const next = splitSizeFromKey(event.key, size, areaHeight);
      // A key this divider does not use belongs to whatever the user is doing
      // in a pane, not to the divider.
      if (next === null) return;
      event.preventDefault();
      onResize(next);
    },
    [areaHeight, onResize, size],
  );

  return (
    <div className="workspace-split" ref={areaRef}>
      <div
        className="workspace-split-pane workspace-split-top"
        data-pane="top"
        style={{ height: `${Math.round(size * 100)}%` }}
      >
        {children}
      </div>
      <div
        className="workspace-split-divider"
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize panes"
        aria-valuemin={Math.round(bounds.min * 100)}
        aria-valuemax={Math.round(bounds.max * 100)}
        aria-valuenow={Math.round(size * 100)}
        tabIndex={0}
        onPointerDown={startDrag}
        onKeyDown={onKeyDown}
      />
      <div className="workspace-split-pane workspace-split-bottom" data-pane="bottom">
        <div className="workspace-split-header">
          <span className="workspace-split-title" title={lowerLabel}>
            {lowerLabel}
          </span>
          {/* Hidden rather than absent, so the keyboard reaches it, and shown on
              hover and on focus alike — see SplitPane.css. */}
          <button type="button" className="workspace-split-merge" onClick={onMerge}>
            Merge into tabs
          </button>
        </div>
        {lower}
      </div>
    </div>
  );
}
