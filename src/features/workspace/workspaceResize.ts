import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent } from "react";

export type ResizeSide = "left" | "right";

/**
 * The shell frame's widths: sidebar 248 (resizable 200–360, collapsible),
 * right panel 300 (its own bounds — the spec pins only the default). A width
 * persisted by an older build, inside the old 180–460 bounds, is clamped into
 * its side's bounds on read.
 */
export const MIN_LEFT_WIDTH = 200;
export const MAX_LEFT_WIDTH = 360;
export const INITIAL_LEFT_WIDTH = 248;
export const MIN_RIGHT_WIDTH = 240;
export const MAX_RIGHT_WIDTH = 420;
export const INITIAL_RIGHT_WIDTH = 300;

const WIDTHS_STORAGE_KEY = "devboule.workspacePanelWidths";

export interface StoredPanelWidths {
  left: number;
  right: number;
}

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

export function clampPanelWidth(width: number, side: ResizeSide): number {
  const min = side === "left" ? MIN_LEFT_WIDTH : MIN_RIGHT_WIDTH;
  const max = side === "left" ? MAX_LEFT_WIDTH : MAX_RIGHT_WIDTH;
  return Math.max(min, Math.min(max, width));
}

function isSideWidth(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

/** Best-effort read; a width outside its bounds (an older build's) is clamped in. */
export function readStoredPanelWidths(storage: StorageLike | null): StoredPanelWidths {
  const defaults: StoredPanelWidths = { left: INITIAL_LEFT_WIDTH, right: INITIAL_RIGHT_WIDTH };
  try {
    const raw = storage?.getItem(WIDTHS_STORAGE_KEY) ?? null;
    if (raw === null) return defaults;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return defaults;
    const row = parsed as Record<string, unknown>;
    return {
      left: isSideWidth(row.left) ? clampPanelWidth(row.left, "left") : defaults.left,
      right: isSideWidth(row.right) ? clampPanelWidth(row.right, "right") : defaults.right,
    };
  } catch {
    return defaults;
  }
}

export function writeStoredPanelWidths(
  storage: StorageLike | null,
  widths: StoredPanelWidths,
): void {
  try {
    storage?.setItem(WIDTHS_STORAGE_KEY, JSON.stringify(widths));
  } catch {
    // Storage can be full or blocked; a lost width must not break the shell.
  }
}

/** The one guarded read of the global: a throwing storage getter must not break the shell. */
function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function useWorkspacePanelResize() {
  const [widths, setWidths] = useState<StoredPanelWidths>(() =>
    readStoredPanelWidths(defaultStorage()),
  );
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);

  const resizeRef = useRef<{ side: ResizeSide; startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    writeStoredPanelWidths(defaultStorage(), widths);
  }, [widths]);

  useEffect(() => {
    const handleMove = (event: globalThis.MouseEvent) => {
      const resize = resizeRef.current;
      if (!resize) return;

      const distance = event.clientX - resize.startX;
      const signedDistance = resize.side === "left" ? distance : -distance;
      const width = clampPanelWidth(resize.startWidth + signedDistance, resize.side);

      setWidths((current) => ({ ...current, [resize.side]: width }));
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
        startWidth: side === "left" ? widths.left : widths.right,
      };
      document.body.classList.add("workspace-is-resizing");
    },
    [widths.left, widths.right],
  );

  const handleResizeKey = useCallback(
    (side: ResizeSide, event: KeyboardEvent<HTMLButtonElement>) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        if (side === "left") setLeftCollapsed((collapsed) => !collapsed);
        else setRightCollapsed((collapsed) => !collapsed);
        return;
      }

      const currentWidth = side === "left" ? widths.left : widths.right;
      let nextWidth: number | null = null;
      if (event.key === "Home") nextWidth = side === "left" ? MIN_LEFT_WIDTH : MIN_RIGHT_WIDTH;
      if (event.key === "End") nextWidth = side === "left" ? MAX_LEFT_WIDTH : MAX_RIGHT_WIDTH;
      if (side === "left" && event.key === "ArrowLeft") nextWidth = currentWidth - 16;
      if (side === "left" && event.key === "ArrowRight") nextWidth = currentWidth + 16;
      if (side === "right" && event.key === "ArrowLeft") nextWidth = currentWidth + 16;
      if (side === "right" && event.key === "ArrowRight") nextWidth = currentWidth - 16;

      if (nextWidth !== null) {
        event.preventDefault();
        const width = clampPanelWidth(nextWidth, side);
        setWidths((current) => ({ ...current, [side]: width }));
      }
    },
    [widths.left, widths.right],
  );

  return {
    leftWidth: widths.left,
    rightWidth: widths.right,
    leftCollapsed,
    rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
    startDrag,
    handleResizeKey,
  };
}
