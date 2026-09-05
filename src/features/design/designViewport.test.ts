import { describe, expect, it, vi } from "vitest";
import {
  createViewport,
  createViewportCommitScheduler,
  fitViewport,
  panViewport,
  pointerToWorld,
  viewportTransform,
  zoomViewport,
} from "./designViewport";

describe("design viewport", () => {
  it("starts with a clamped zoom and an explicit pan", () => {
    expect(createViewport(4, { x: 12, y: -8 })).toEqual({
      zoom: 3,
      pan: { x: 12, y: -8 },
    });
  });

  it("pans without changing zoom", () => {
    expect(panViewport({ zoom: 1, pan: { x: 10, y: 20 } }, { x: -5, y: 8 })).toEqual({
      zoom: 1,
      pan: { x: 5, y: 28 },
    });
  });

  it("keeps the world point under the pointer fixed while zooming", () => {
    const viewport = createViewport(1, { x: 30, y: -12 });
    const before = pointerToWorld(240, 160, { left: 0, top: 0 }, viewport);
    const next = zoomViewport(viewport, { deltaY: -1, deltaMode: 0 }, 240, 160, 600);

    expect(pointerToWorld(240, 160, { left: 0, top: 0 }, next)).toEqual(before);
  });

  it("scales wheel zoom by delta magnitude and normalizes line deltas", () => {
    const viewport = createViewport();
    const small = zoomViewport(viewport, { deltaY: -2, deltaMode: 0 }, 0, 0, 600);
    const large = zoomViewport(viewport, { deltaY: -8, deltaMode: 0 }, 0, 0, 600);
    const capped = zoomViewport(viewport, { deltaY: -100, deltaMode: 0 }, 0, 0, 600);
    const line = zoomViewport(viewport, { deltaY: -1, deltaMode: 1 }, 0, 0, 600);
    const page = zoomViewport(viewport, { deltaY: -1, deltaMode: 2 }, 0, 0, 600);

    expect(large.zoom).toBeGreaterThan(small.zoom);
    expect(capped.zoom).toBe(zoomViewport(viewport, { deltaY: -10, deltaMode: 0 }, 0, 0, 600).zoom);
    expect(line.zoom).toBeGreaterThan(viewport.zoom);
    expect(line.zoom).toBe(capped.zoom);
    expect(page.zoom).toBe(capped.zoom);
  });

  it("converts client coordinates into world coordinates", () => {
    expect(
      pointerToWorld(210, 130, { left: 10, top: 10 }, { zoom: 2, pan: { x: 20, y: 30 } }),
    ).toEqual({
      x: 90,
      y: 45,
    });
  });

  it("fits with the same pan convention while using the 3x range", () => {
    expect(fitViewport({ x: 0, y: 0, w: 100, h: 100 }, 800, 600)).toEqual({
      zoom: 3,
      pan: { x: 250, y: 150 },
    });
  });

  it("formats a single world container transform", () => {
    expect(viewportTransform({ zoom: 1.25, pan: { x: 18, y: -6 } })).toBe(
      "translate(18px, -6px) scale(1.25)",
    );
  });

  it("commits only the latest viewport once for a burst of wheel events", () => {
    let frame: (() => void) | undefined;
    const commit = vi.fn();
    const scheduler = createViewportCommitScheduler(
      commit,
      (callback) => {
        frame = callback;
        return 1;
      },
      vi.fn(),
    );
    const first = createViewport(1);
    const second = createViewport(1.2);

    scheduler.schedule(first);
    scheduler.schedule(second);

    expect(commit).not.toHaveBeenCalled();
    frame?.();
    expect(commit).toHaveBeenCalledTimes(1);
    expect(commit).toHaveBeenCalledWith(second);
  });

  it("cancels a pending viewport commit", () => {
    let frame: (() => void) | undefined;
    const commit = vi.fn();
    const cancelFrame = vi.fn();
    const scheduler = createViewportCommitScheduler(
      commit,
      (callback) => {
        frame = callback;
        return 7;
      },
      cancelFrame,
    );

    scheduler.schedule(createViewport(1.2));
    scheduler.cancel();
    frame?.();

    expect(cancelFrame).toHaveBeenCalledWith(7);
    expect(commit).not.toHaveBeenCalled();
  });
});
