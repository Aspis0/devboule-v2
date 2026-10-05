import { useCallback, useEffect, useRef, useState, type SetStateAction } from "react";
import type { KeyboardEvent, MouseEvent } from "react";

export type ResizeSide = "left" | "right";

/**
 * The shell frame's widths: sidebar 320 (resizable 200–600, collapsible),
 * right panel 300 (its own bounds — the spec pins only the default). A width
 * persisted by an older build, outside the current bounds, is clamped into
 * its side's bounds on read.
 */
export const MIN_LEFT_WIDTH = 200;
export const MAX_LEFT_WIDTH = 600;
export const INITIAL_LEFT_WIDTH = 320;
export const MIN_RIGHT_WIDTH = 240;
export const MAX_RIGHT_WIDTH = 420;
export const INITIAL_RIGHT_WIDTH = 300;

const WIDTHS_STORAGE_KEY = "devboule.workspacePanelWidths";

/** The shell frame as the one guarded record holds it: both widths and
 *  both collapsed flags. The flags are optional on disk — a record from
 *  before them (or one whose flag is not a boolean) reads as open, with
 *  its widths untouched. */
export interface StoredPanelFrame {
  left: number;
  right: number;
  leftCollapsed: boolean;
  rightCollapsed: boolean;
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

/** Best-effort read: a width outside its bounds (an older build's) is
 *  clamped in; a missing or invalid collapsed flag reads as open. */
export function readStoredPanelFrame(storage: StorageLike | null): StoredPanelFrame {
  const defaults: StoredPanelFrame = {
    left: INITIAL_LEFT_WIDTH,
    right: INITIAL_RIGHT_WIDTH,
    leftCollapsed: false,
    rightCollapsed: false,
  };
  try {
    const raw = storage?.getItem(WIDTHS_STORAGE_KEY) ?? null;
    if (raw === null) return defaults;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return defaults;
    const row = parsed as Record<string, unknown>;
    return {
      left: isSideWidth(row.left) ? clampPanelWidth(row.left, "left") : defaults.left,
      right: isSideWidth(row.right) ? clampPanelWidth(row.right, "right") : defaults.right,
      leftCollapsed: typeof row.leftCollapsed === "boolean" ? row.leftCollapsed : false,
      rightCollapsed: typeof row.rightCollapsed === "boolean" ? row.rightCollapsed : false,
    };
  } catch {
    return defaults;
  }
}

export function writeStoredPanelFrame(storage: StorageLike | null, frame: StoredPanelFrame): void {
  try {
    storage?.setItem(WIDTHS_STORAGE_KEY, JSON.stringify(frame));
  } catch {
    // Storage can be full or blocked; a lost frame must not break the shell.
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
  const [frame, setFrame] = useState<StoredPanelFrame>(() =>
    readStoredPanelFrame(defaultStorage()),
  );

  const resizeRef = useRef<{ side: ResizeSide; startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    writeStoredPanelFrame(defaultStorage(), frame);
  }, [frame]);

  useEffect(() => {
    const handleMove = (event: globalThis.MouseEvent) => {
      const resize = resizeRef.current;
      if (!resize) return;

      const distance = event.clientX - resize.startX;
      const signedDistance = resize.side === "left" ? distance : -distance;
      const width = clampPanelWidth(resize.startWidth + signedDistance, resize.side);

      setFrame((current) => ({ ...current, [resize.side]: width }));
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

  // One setter per side, shared by every route in (menu collapse, the strip's
  // expand button, double-click, Enter), so one write carries all four fields.
  const setCollapsed = useCallback((side: ResizeSide, action: SetStateAction<boolean>) => {
    setFrame((current) => {
      const key = side === "left" ? "leftCollapsed" : "rightCollapsed";
      const value = typeof action === "function" ? action(current[key]) : action;
      return { ...current, [key]: value };
    });
  }, []);

  const startDrag = useCallback(
    (side: ResizeSide, event: MouseEvent<HTMLButtonElement>) => {
      event.preventDefault();
      resizeRef.current = {
        side,
        startX: event.clientX,
        startWidth: side === "left" ? frame.left : frame.right,
      };
      document.body.classList.add("workspace-is-resizing");
    },
    [frame.left, frame.right],
  );

  const handleResizeKey = useCallback(
    (side: ResizeSide, event: KeyboardEvent<HTMLButtonElement>) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        setCollapsed(side, (collapsed) => !collapsed);
        return;
      }

      const currentWidth = side === "left" ? frame.left : frame.right;
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
        setFrame((current) => ({ ...current, [side]: width }));
      }
    },
    [frame.left, frame.right, setCollapsed],
  );

  const setLeftCollapsed = useCallback(
    (action: SetStateAction<boolean>) => setCollapsed("left", action),
    [setCollapsed],
  );
  const setRightCollapsed = useCallback(
    (action: SetStateAction<boolean>) => setCollapsed("right", action),
    [setCollapsed],
  );

  return {
    leftWidth: frame.left,
    rightWidth: frame.right,
    leftCollapsed: frame.leftCollapsed,
    rightCollapsed: frame.rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
    startDrag,
    handleResizeKey,
  };
}
