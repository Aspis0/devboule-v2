// Why the preview is an overlay the placement knows about: the page in the pane
// below is a child webview in Rust — a native window above this app's own — so a
// DOM preview drawn under it would be invisible, and the page would sit on top of
// the thing the drag is saying. Registering the preview is how the page is told
// to get out of the way, and unregistering it (the drop) is how the page comes
// back, once, at the rectangle the pane now measures.

import { useEffect, useRef } from "react";
import { registerOverlay } from "../browserOverlays";
import type { DropZone } from "./tabDropZones";

export interface SplitDropPreviewProps {
  /** Where the drop would land, or null while the drag has not left the chip.
   * `strip` draws nothing: the pointer is over the row the tab came from. */
  zone: DropZone | "strip" | null;
}

/** The destination, drawn over the pane the drop is resolved against: an edge
 * takes half the pane, the centre takes the whole of it, because a drop there
 * changes nothing and says so. */
export function SplitDropPreview({ zone }: SplitDropPreviewProps) {
  const ref = useRef<HTMLDivElement>(null);
  const shown = zone !== null && zone !== "strip";

  useEffect(() => {
    if (!shown) return;
    return registerOverlay(ref.current);
  }, [shown]);

  if (!shown) return null;
  return (
    <div className="workspace-drop-layer" aria-hidden="true">
      <div className="workspace-drop-preview" ref={ref} data-zone={zone} />
    </div>
  );
}
