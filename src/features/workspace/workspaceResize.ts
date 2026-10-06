import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type SetStateAction,
} from "react";
import type { KeyboardEvent, MouseEvent } from "react";

export type ResizeSide = "left" | "right";

/**
 * The shell frame's widths: sidebar 248 (resizable 200–360, collapsible),
 * right panel 300 (its own bounds — the spec pins only the default). A width
 * persisted by an older build, outside the current bounds, is clamped into
 * its side's bounds on read, and the two panels share what the window leaves
 * after the centre's floor.
 */
export const MIN_LEFT_WIDTH = 200;
export const MAX_LEFT_WIDTH = 360;
export const INITIAL_LEFT_WIDTH = 248;
export const MIN_RIGHT_WIDTH = 240;
export const MAX_RIGHT_WIDTH = 420;
export const INITIAL_RIGHT_WIDTH = 300;

/** The centre pane keeps at least this much of the window: at 1024px the rail
 *  stops at 352 instead of pushing the right panel off the screen. */
export const MIN_CENTER_WIDTH = 360;

/** The two 6px tracks between the panels and the centre. */
const TRACK_TOTAL = 12;

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

/** What the window still owes the other panel: the room a width is judged in. */
export interface PanelRoom {
  other: number;
  viewport: number;
}

/**
 * Bounds first, then the window: a panel may not eat the centre's floor, so
 * the clamp that answers "how wide may this side be" also knows what the other
 * side already holds. A window too small for both minima keeps the side's own
 * minimum — the floor gives way there, not the panel.
 */
export function clampPanelWidth(width: number, side: ResizeSide, room?: PanelRoom): number {
  const min = side === "left" ? MIN_LEFT_WIDTH : MIN_RIGHT_WIDTH;
  const max = side === "left" ? MAX_LEFT_WIDTH : MAX_RIGHT_WIDTH;
  const bounded = Math.max(min, Math.min(max, width));
  if (room === undefined || room.viewport <= 0) return bounded;
  const ceiling = room.viewport - room.other - MIN_CENTER_WIDTH - TRACK_TOTAL;
  return Math.max(min, Math.min(bounded, ceiling));
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
  const [viewport, setViewport] = useState(() =>
    typeof window === "undefined" ? 0 : window.innerWidth,
  );

  const resizeRef = useRef<{ side: ResizeSide; startX: number; startWidth: number } | null>(null);
  // The drag listeners stay mounted for the whole gesture — their cleanup also
  // drops the cursor class — so they read the frame and the window from here.
  // A layout effect, so the ref holds the committed frame before any pointer
  // event can read it.
  const roomRef = useRef<{ frame: StoredPanelFrame; viewport: number }>({
    frame,
    viewport,
  });
  useLayoutEffect(() => {
    roomRef.current = { frame, viewport };
  }, [frame, viewport]);

  useEffect(() => {
    const onResize = () => setViewport(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    writeStoredPanelFrame(defaultStorage(), frame);
  }, [frame]);

  // What the screen draws: the record keeps the width the user chose, the
  // layout keeps the centre's floor at this window.
  const leftRoom: PanelRoom = { other: frame.right, viewport };
  const leftWidth = clampPanelWidth(frame.left, "left", leftRoom);
  const rightWidth = clampPanelWidth(frame.right, "right", { other: leftWidth, viewport });
  const leftMax = clampPanelWidth(MAX_LEFT_WIDTH, "left", leftRoom);
  const rightMax = clampPanelWidth(MAX_RIGHT_WIDTH, "right", { other: leftWidth, viewport });

  useEffect(() => {
    const handleMove = (event: globalThis.MouseEvent) => {
      const resize = resizeRef.current;
      if (!resize) return;

      const distance = event.clientX - resize.startX;
      const signedDistance = resize.side === "left" ? distance : -distance;
      const latest = roomRef.current;
      const room: PanelRoom = {
        other: resize.side === "left" ? latest.frame.right : latest.frame.left,
        viewport: latest.viewport,
      };
      const width = clampPanelWidth(resize.startWidth + signedDistance, resize.side, room);

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
        // The width on screen, not the record's: a drag has to move the first
        // pixel the pointer moves, even when the window had clipped the side.
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
        setCollapsed(side, (collapsed) => !collapsed);
        return;
      }

      const currentWidth = side === "left" ? leftWidth : rightWidth;
      let nextWidth: number | null = null;
      if (event.key === "Home") nextWidth = side === "left" ? MIN_LEFT_WIDTH : MIN_RIGHT_WIDTH;
      if (event.key === "End") nextWidth = side === "left" ? MAX_LEFT_WIDTH : MAX_RIGHT_WIDTH;
      if (side === "left" && event.key === "ArrowLeft") nextWidth = currentWidth - 16;
      if (side === "left" && event.key === "ArrowRight") nextWidth = currentWidth + 16;
      if (side === "right" && event.key === "ArrowLeft") nextWidth = currentWidth + 16;
      if (side === "right" && event.key === "ArrowRight") nextWidth = currentWidth - 16;

      if (nextWidth !== null) {
        event.preventDefault();
        const room: PanelRoom = {
          other: side === "left" ? frame.right : leftWidth,
          viewport,
        };
        const width = clampPanelWidth(nextWidth, side, room);
        setFrame((current) => ({ ...current, [side]: width }));
      }
    },
    [leftWidth, rightWidth, frame.right, viewport, setCollapsed],
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
    leftWidth,
    rightWidth,
    leftMax,
    rightMax,
    leftCollapsed: frame.leftCollapsed,
    rightCollapsed: frame.rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
    startDrag,
    handleResizeKey,
  };
}
