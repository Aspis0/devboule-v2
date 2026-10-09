import type { TabInsertionMark } from "./useTabDrag";
import "./TabInsertionLine.css";

/** The gap a dragged tab would land in on the tab row, drawn as one thin line. */
export function TabInsertionLine({ mark }: { mark: TabInsertionMark | null }) {
  if (mark === null) return null;
  return (
    <div
      className="workspace-tab-insertion"
      style={{ left: `${mark.x}px`, top: `${mark.top}px`, height: `${mark.height}px` }}
      aria-hidden="true"
    />
  );
}
