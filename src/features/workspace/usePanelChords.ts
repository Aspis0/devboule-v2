import { useEffect } from "react";
import { commandModifier } from "../../lib/keymap";
import { panelChordFor } from "../../lib/panelChords";

interface PanelChordsOptions {
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
  setRightCollapsed: (collapsed: boolean) => void;
}

/**
 * Window-level panel toggles. "Both" collapses the two when either is open,
 * and opens them when both are closed.
 */
export function usePanelChords({
  leftCollapsed,
  rightCollapsed,
  setLeftCollapsed,
  setRightCollapsed,
}: PanelChordsOptions): void {
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const chord = panelChordFor(event, commandModifier() === "Cmd");
      if (chord === null) return;
      event.preventDefault();
      if (chord === "sidebar") {
        setLeftCollapsed(!leftCollapsed);
      } else if (chord === "panel") {
        setRightCollapsed(!rightCollapsed);
      } else {
        const collapse = !leftCollapsed || !rightCollapsed;
        setLeftCollapsed(collapse);
        setRightCollapsed(collapse);
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [leftCollapsed, rightCollapsed, setLeftCollapsed, setRightCollapsed]);
}
