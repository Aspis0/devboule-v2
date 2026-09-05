// Wheel adaptation follows the approach in plat's MIT src/camera.ts. No plat
// source is copied here: the bounded additive step and unit conversion are
// implemented for this surface's viewport contract.
import {
  fitToBounds,
  screenToWorld,
  zoomAtPoint,
  type Bounds,
  type Pan,
} from "../../lib/canvas/viewportMath";
import type { Point } from "../../types/geometry";

export const DESIGN_MIN_ZOOM = 0.2;
export const DESIGN_MAX_ZOOM = 3;
const WHEEL_DELTA_LIMIT = 10;
const WHEEL_LINE_HEIGHT = 16;

export function clampViewportZoom(zoom: number): number {
  if (Number.isNaN(zoom)) return DESIGN_MIN_ZOOM;
  return Math.min(DESIGN_MAX_ZOOM, Math.max(DESIGN_MIN_ZOOM, zoom));
}

/** Keep each wheel event bounded so trackpad deltas do not compound too fast. */
export function wheelZoomDelta(deltaY: number): number {
  const bounded = Math.max(-WHEEL_DELTA_LIMIT, Math.min(WHEEL_DELTA_LIMIT, deltaY));
  return -bounded / 100;
}

/** Convert browser line/page wheel units into container-relative pixels. */
export function normalizeWheel(deltaY: number, deltaMode: number, viewportHeight: number): number {
  if (deltaMode === 1) return deltaY * WHEEL_LINE_HEIGHT;
  if (deltaMode === 2) return deltaY * viewportHeight;
  return deltaY;
}

export interface DesignViewport {
  pan: Pan;
  zoom: number;
}

export interface ViewportClientRect {
  left: number;
  top: number;
}

export interface WheelInput {
  deltaY: number;
  deltaMode: number;
}

export interface ViewportCommitScheduler {
  schedule(viewport: DesignViewport): void;
  flush(viewport?: DesignViewport): void;
  cancel(): void;
}

export function createViewport(zoom = 1, pan: Pan = { x: 0, y: 0 }): DesignViewport {
  return { zoom: clampViewportZoom(zoom), pan: { ...pan } };
}

export function panViewport(viewport: DesignViewport, delta: Pan): DesignViewport {
  return {
    zoom: viewport.zoom,
    pan: { x: viewport.pan.x + delta.x, y: viewport.pan.y + delta.y },
  };
}

export function zoomViewport(
  viewport: DesignViewport,
  wheel: WheelInput,
  clientX: number,
  clientY: number,
  viewportHeight: number,
): DesignViewport {
  const delta = wheelZoomDelta(normalizeWheel(wheel.deltaY, wheel.deltaMode, viewportHeight));
  const nextZoom = clampViewportZoom(viewport.zoom + delta * viewport.zoom);
  return {
    zoom: nextZoom,
    pan: zoomAtPoint(viewport.zoom, viewport.pan, nextZoom, clientX, clientY),
  };
}

export function pointerToWorld(
  clientX: number,
  clientY: number,
  rect: ViewportClientRect,
  viewport: DesignViewport,
): Point {
  return screenToWorld(clientX - rect.left, clientY - rect.top, viewport.pan, viewport.zoom);
}

export function fitViewport(
  bounds: Bounds | null,
  width: number,
  height: number,
  margin?: number,
): DesignViewport {
  if (!bounds || bounds.w <= 0 || bounds.h <= 0) {
    const fitted = fitToBounds(bounds, width, height, margin);
    return { ...fitted, zoom: clampViewportZoom(fitted.zoom) };
  }

  // The ported engine's fit helper has a separate 2x clamp. Derive the valid
  // fit here so every Design viewport uses the same [0.2, 3] range and pan
  // convention as the interactive zoom path.
  const fitMargin = margin ?? 80;
  const availableWidth = Math.max(1, width - fitMargin * 2);
  const availableHeight = Math.max(1, height - fitMargin * 2);
  const zoom = clampViewportZoom(Math.min(availableWidth / bounds.w, availableHeight / bounds.h));
  return {
    zoom,
    pan: {
      x: (width - bounds.w * zoom) / 2 - bounds.x * zoom,
      y: (height - bounds.h * zoom) / 2 - bounds.y * zoom,
    },
  };
}

export function viewportTransform(viewport: DesignViewport): string {
  return `translate(${viewport.pan.x}px, ${viewport.pan.y}px) scale(${viewport.zoom})`;
}

export function createViewportCommitScheduler(
  commit: (viewport: DesignViewport) => void,
  scheduleFrame: (callback: () => void) => number,
  cancelFrame: (frameId: number) => void,
): ViewportCommitScheduler {
  let pending: DesignViewport | null = null;
  let frameId: number | null = null;

  const commitPending = () => {
    frameId = null;
    const next = pending;
    pending = null;
    if (next !== null) commit(next);
  };

  return {
    schedule(viewport) {
      pending = viewport;
      if (frameId === null) frameId = scheduleFrame(commitPending);
    },
    flush(viewport) {
      if (frameId !== null) cancelFrame(frameId);
      frameId = null;
      const next = viewport ?? pending;
      pending = null;
      if (next !== undefined && next !== null) commit(next);
    },
    cancel() {
      if (frameId !== null) cancelFrame(frameId);
      frameId = null;
      pending = null;
    },
  };
}
