// Why the gesture lives here and not in the workspace: a drag reads two boxes
// and writes one zone, and the surface above it has no other business knowing
// where the pointer is. Only a zone the preview has to draw is state, so a
// pointer moving inside one destination re-renders this layer and nothing above
// it, and the strip's press reaches the gesture through this one handle.

import { forwardRef, useImperativeHandle } from "react";
import { SplitDropPreview } from "./SplitDropPreview";
import { TabInsertionLine } from "./TabInsertionLine";
import { tabCanGoBelow, type DropZone } from "./tabDropZones";
import {
  useTabDrag,
  type TabDragBoxes,
  type TabDropPoint,
  type TabInsertionMark,
} from "./useTabDrag";

/** What the strip's press needs to hand the gesture, and no more. */
export interface SplitDragLayerHandle {
  start: (
    tabId: string,
    owner: Element,
    event: { clientX: number; clientY: number; pointerId: number },
  ) => void;
}

export interface SplitDragLayerProps {
  /** The two boxes a drop is read against, read live. */
  boxes: () => TabDragBoxes;
  /** Whether the tab is still open: a tab an agent closes ends the gesture. */
  hasTab: (tabId: string) => boolean;
  /** Where a tab dropped on the tab row would land, for the line that marks it. */
  markAt: (tabId: string, point: TabDropPoint) => TabInsertionMark | null;
  /** What a drop does, decided by the caller against the live panes. */
  onDrop: (tabId: string, zone: DropZone | "strip", point: TabDropPoint) => void;
}

/** The drag, and the destination it is previewing over the workspace centre. */
export const SplitDragLayer = forwardRef<SplitDragLayerHandle, SplitDragLayerProps>(
  function SplitDragLayer({ boxes, hasTab, markAt, onDrop }, ref) {
    const { zone, tabId, mark, startDrag } = useTabDrag({ boxes, hasTab, markAt, onDrop });
    useImperativeHandle(ref, () => ({ start: startDrag }), [startDrag]);
    // Only a tab the pane below can hold is previewed over the centre: any other
    // tab dropped there is a selection, and a preview would promise a split.
    const previewed = tabId !== null && tabCanGoBelow(tabId) ? zone : null;
    return (
      <>
        <SplitDropPreview zone={previewed} />
        <TabInsertionLine mark={zone === "strip" ? mark : null} />
      </>
    );
  },
);
