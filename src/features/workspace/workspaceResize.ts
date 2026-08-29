import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent } from "react";

export type ResizeSide = "left" | "right";

export const MIN_PANEL_WIDTH = 180;
export const MAX_PANEL_WIDTH = 460;
export const INITIAL_LEFT_WIDTH = 252;
export const INITIAL_RIGHT_WIDTH = 366;

export function clampPanelWidth(width: number): number {
  return Math.max(MIN_PANEL_WIDTH, Math.min(MAX_PANEL_WIDTH, width));
}

export function useWorkspacePanelResize() {
  const [leftWidth, setLeftWidth] = useState(INITIAL_LEFT_WIDTH);
  const [rightWidth, setRightWidth] = useState(INITIAL_RIGHT_WIDTH);
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);

  const resizeRef = useRef<{ side: ResizeSide; startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    const handleMove = (event: globalThis.MouseEvent) => {
      const resize = resizeRef.current;
      if (!resize) return;

      const distance = event.clientX - resize.startX;
      const signedDistance = resize.side === "left" ? distance : -distance;
      const width = clampPanelWidth(resize.startWidth + signedDistance);

      if (resize.side === "left") {
        setLeftWidth(width);
      } else {
        setRightWidth(width);
      }
    };

    const handleUp = () => {
      resizeRef.current = null;
      document.body.classList.remove("workspace-is-resizing");
    };

    document.addEventListener("mousemove", handleMove);
    document.addEventListener("mouseup", handleUp);
    return () => {
      document.removeEventListener("mousemove", handleMove);
      document.removeEventListener("mouseup", handleUp);
      document.body.classList.remove("workspace-is-resizing");
    };
  }, []);

  const startDrag = useCallback(
    (side: ResizeSide, event: MouseEvent<HTMLButtonElement>) => {
      event.preventDefault();
      resizeRef.current = {
        side,
        startX: event.clientX,
        startWidth: side === "left" ? leftWidth : rightWidth,
      };
      document.body.classList.add("workspace-is-resizing");
    },
    [leftWidth, rightWidth],
  );

  const handleResizeKey = useCallback(
    (side: ResizeSide, event: KeyboardEvent<HTMLButtonElement>) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        if (side === "left") setLeftCollapsed((collapsed) => !collapsed);
        else setRightCollapsed((collapsed) => !collapsed);
        return;
      }

      const currentWidth = side === "left" ? leftWidth : rightWidth;
      let nextWidth: number | null = null;
      if (event.key === "Home") nextWidth = MIN_PANEL_WIDTH;
      if (event.key === "End") nextWidth = MAX_PANEL_WIDTH;
      if (side === "left" && event.key === "ArrowLeft") nextWidth = currentWidth - 16;
      if (side === "left" && event.key === "ArrowRight") nextWidth = currentWidth + 16;
      if (side === "right" && event.key === "ArrowLeft") nextWidth = currentWidth + 16;
      if (side === "right" && event.key === "ArrowRight") nextWidth = currentWidth - 16;

      if (nextWidth !== null) {
        event.preventDefault();
        const width = clampPanelWidth(nextWidth);
        if (side === "left") setLeftWidth(width);
        else setRightWidth(width);
      }
    },
    [leftWidth, rightWidth],
  );

  return {
    leftWidth,
    rightWidth,
    leftCollapsed,
    rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
    startDrag,
    handleResizeKey,
  };
}
