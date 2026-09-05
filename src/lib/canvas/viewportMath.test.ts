// Ported from PubSpark's src/canvas/engine/viewportMath.test.ts.
import { describe, it, expect } from "vitest";
import {
  clampZoom,
  zoomAtPoint,
  wheelZoom,
  worldToScreen,
  screenToWorld,
  nodesBounds,
  fitTargetBounds,
  fitToBounds,
  boundsVisible,
  revealView,
  MIN_ZOOM,
  MAX_ZOOM,
} from "./viewportMath";
import type { NodeRect } from "../../types/geometry";

function rect(over: Partial<NodeRect> = {}): NodeRect {
  return { id: "n", x: 0, y: 0, w: 100, h: 100, z: 1, ...over };
}

describe("clampZoom", () => {
  it("clamps below MIN and above MAX", () => {
    expect(clampZoom(0.01)).toBe(MIN_ZOOM);
    expect(clampZoom(5)).toBe(MAX_ZOOM);
    expect(clampZoom(1)).toBe(1);
  });
  it("clamps NaN to MIN", () => {
    expect(clampZoom(Number.NaN)).toBe(MIN_ZOOM);
  });
});

describe("zoomAtPoint — cursor-anchored zoom keeps the world point under the cursor", () => {
  it("keeps the world coordinate under the cursor fixed on screen", () => {
    const zoom = 1;
    const pan = { x: 50, y: 20 };
    const cx = 200;
    const cy = 120;
    // World point currently under the cursor.
    const before = screenToWorld(cx, cy, pan, zoom);
    const newZoom = 1.6;
    const newPan = zoomAtPoint(zoom, pan, newZoom, cx, cy);
    // That same world point must still project to the cursor at the new zoom.
    const screen = worldToScreen(before.x, before.y, newPan, newZoom);
    expect(screen.x).toBeCloseTo(cx, 6);
    expect(screen.y).toBeCloseTo(cy, 6);
  });
});

describe("wheelZoom", () => {
  it("zooms in on wheel-up (deltaY<0) and clamps", () => {
    const r = wheelZoom(1, { x: 0, y: 0 }, -1, 100, 100);
    expect(r.zoom).toBeGreaterThan(1);
    expect(r.zoom).toBeLessThanOrEqual(MAX_ZOOM);
  });
  it("zooms out on wheel-down and never below MIN", () => {
    let z = 1;
    let pan = { x: 0, y: 0 };
    for (let i = 0; i < 50; i++) {
      const r = wheelZoom(z, pan, 1, 100, 100);
      z = r.zoom;
      pan = r.pan;
    }
    expect(z).toBe(MIN_ZOOM);
  });
});

describe("worldToScreen / screenToWorld round-trip", () => {
  it("is an exact inverse", () => {
    const pan = { x: 33, y: -12 };
    const zoom = 0.75;
    const w = screenToWorld(400, 250, pan, zoom);
    const s = worldToScreen(w.x, w.y, pan, zoom);
    expect(s.x).toBeCloseTo(400, 6);
    expect(s.y).toBeCloseTo(250, 6);
  });
});

describe("nodesBounds", () => {
  it("returns null for an empty set", () => {
    expect(nodesBounds([])).toBeNull();
  });
  it("computes the union bounding box", () => {
    const b = nodesBounds([
      rect({ x: 10, y: 20, w: 100, h: 50 }),
      rect({ x: 200, y: 0, w: 40, h: 300 }),
    ]);
    expect(b).toEqual({ x: 10, y: 0, w: 230, h: 300 });
  });
});

describe("fitToBounds", () => {
  it("returns the default view for null/degenerate bounds (no divide-by-zero)", () => {
    expect(fitToBounds(null, 800, 600)).toEqual({
      zoom: 0.85,
      pan: { x: 40, y: 24 },
    });
    expect(fitToBounds({ x: 0, y: 0, w: 0, h: 0 }, 800, 600).zoom).toBe(0.85);
  });

  it("fits the bounds within the viewport (clamped) and centers them", () => {
    const bounds = { x: 0, y: 0, w: 1000, h: 500 };
    const vw = 800;
    const vh = 600;
    const margin = 80;
    const { zoom, pan } = fitToBounds(bounds, vw, vh, margin);
    // Limiting axis is width: (800-160)/1000 = 0.64.
    expect(zoom).toBeCloseTo(0.64, 6);
    // Scaled box is centered: its screen top-left + half = viewport center.
    const scaledW = bounds.w * zoom;
    const scaledH = bounds.h * zoom;
    const centerX = pan.x + bounds.x * zoom + scaledW / 2;
    const centerY = pan.y + bounds.y * zoom + scaledH / 2;
    expect(centerX).toBeCloseTo(vw / 2, 6);
    expect(centerY).toBeCloseTo(vh / 2, 6);
  });

  it("clamps the fit zoom to MAX for a tiny bounds", () => {
    const { zoom } = fitToBounds({ x: 0, y: 0, w: 10, h: 10 }, 800, 600);
    expect(zoom).toBe(MAX_ZOOM);
  });
});

describe("fitTargetBounds", () => {
  const ab = (id: string, x: number, y: number, w: number, h: number) => ({ id, x, y, w, h });
  const fallback = { x: 0, y: 0, w: 800, h: 600 };
  const vw = 1000;
  const vh = 800;
  const margin = 60;

  it("with three artboards and no selection frames the union of all three", () => {
    const boards = [
      ab("a1", 0, 0, 800, 600),
      ab("a2", 900, 0, 800, 600),
      ab("a3", 1800, 0, 800, 600),
    ];
    const bounds = fitTargetBounds(boards, null, fallback);
    expect(bounds).toEqual({ x: 0, y: 0, w: 2600, h: 600 });
    // Same view Fit would apply through fitToBounds
    const view = fitToBounds(bounds, vw, vh, margin);
    const expected = fitToBounds(nodesBounds(boards.map((b) => ({ ...b, z: 0 }))), vw, vh, margin);
    expect(view).toEqual(expected);
  });

  it("with artboard 2 selected frames only artboard 2", () => {
    const boards = [
      ab("a1", 0, 0, 800, 600),
      ab("a2", 900, 0, 800, 600),
      ab("a3", 1800, 0, 800, 600),
    ];
    const bounds = fitTargetBounds(boards, "a2", fallback);
    expect(bounds).toEqual({ x: 900, y: 0, w: 800, h: 600 });
    expect(fitToBounds(bounds, vw, vh, margin)).toEqual(
      fitToBounds({ x: 900, y: 0, w: 800, h: 600 }, vw, vh, margin),
    );
  });

  it("with a single artboard frames that board, not the fallback", () => {
    // Lone board whose rect differs from fallback — pins "a lone board frames
    // itself". An always-fallback implementation would pass the old tautology.
    const boards = [ab("a1", 40, 20, 500, 400)];
    const boardRect = { x: 40, y: 20, w: 500, h: 400 };
    expect(fitTargetBounds(boards, null, fallback)).toEqual(boardRect);
    expect(fitTargetBounds(boards, "a1", fallback)).toEqual(boardRect);
  });

  it("with a stale/nonexistent selected id falls through to the union", () => {
    const boards = [ab("a1", 0, 0, 800, 600), ab("a2", 900, 0, 800, 600)];
    // Reachable for one render before selection is pruned.
    expect(fitTargetBounds(boards, "gone", fallback)).toEqual({
      x: 0,
      y: 0,
      w: 1700,
      h: 600,
    });
  });

  it("with an empty artboard list and a selected id falls back", () => {
    expect(fitTargetBounds([], "a1", fallback)).toEqual(fallback);
  });
});

// Viewport is 800×600; padFrac 0.15 → comfortable region x∈[120,680], y∈[90,510].
describe("boundsVisible", () => {
  it("is true when the whole bbox sits inside the inset region", () => {
    const b = { x: 200, y: 100, w: 100, h: 100 };
    expect(boundsVisible(b, { x: 0, y: 0 }, 1, 800, 600)).toBe(true);
  });
  it("is false when the bbox overhangs the padding edge", () => {
    const b = { x: 700, y: 100, w: 100, h: 100 }; // x1 = 800 > 680
    expect(boundsVisible(b, { x: 0, y: 0 }, 1, 800, 600)).toBe(false);
  });
});

describe("revealView", () => {
  it("does nothing when the bbox is already comfortably visible", () => {
    const b = { x: 200, y: 100, w: 100, h: 100 };
    const pan = { x: 0, y: 0 };
    const r = revealView(b, pan, 1, 800, 600);
    expect(r).toEqual({ zoom: 1, pan: { x: 0, y: 0 } });
  });

  it("pans the minimal amount (keeping zoom) to bring an off-screen bbox inside", () => {
    const b = { x: 1000, y: 100, w: 100, h: 100 }; // fits (100 ≤ 560) but far right
    const r = revealView(b, { x: 0, y: 0 }, 1, 800, 600);
    expect(r.zoom).toBe(1); // never zooms in to reveal
    // Far-edge overhang: shift so x1 lands on 680 → pan.x = 0 - (1100 - 680).
    expect(r.pan.x).toBe(-420);
    expect(r.pan.y).toBe(0); // y already inside → untouched
  });

  it("never zooms IN even when the bbox is tiny", () => {
    const b = { x: 2000, y: 2000, w: 10, h: 10 };
    const r = revealView(b, { x: 0, y: 0 }, 1, 800, 600);
    expect(r.zoom).toBe(1); // stays put, unlike fitToBounds which would magnify
  });

  it("zooms OUT and centres when the bbox does not fit at the current zoom", () => {
    // 12000×6000 at pad 0.15 → fit zoom min(560/12000, 420/6000) ≈ 0.0467, under MIN.
    const b = { x: 0, y: 0, w: 12000, h: 6000 };
    const r = revealView(b, { x: 0, y: 0 }, 1, 800, 600);
    expect(r.zoom).toBeLessThan(1); // zoomed out to fit
    expect(r.zoom).toBe(MIN_ZOOM); // clamped
    // Centred at MIN_ZOOM: pan = ((800-600)/2, (600-300)/2).
    expect(r.pan.x).toBeCloseTo(100, 6);
    expect(r.pan.y).toBeCloseTo(150, 6);
  });
});
