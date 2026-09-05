// Ported from PubSpark's src/canvas/engine/multiResize.test.ts.
import { describe, expect, it } from "vitest";
import { multiResize, type MemberRect, type UnionRect } from "./multiResize";

/** Convenience constructor so each test reads as data, not boilerplate. */
function member(over: Partial<MemberRect> & { id: string }): MemberRect {
  return {
    id: over.id,
    x: over.x ?? 0,
    y: over.y ?? 0,
    w: over.w ?? 100,
    h: over.h ?? 100,
    derivedSize: over.derivedSize ?? false,
  };
}

function union(over: Partial<UnionRect> = {}): UnionRect {
  return { x: over.x ?? 0, y: over.y ?? 0, w: over.w ?? 100, h: over.h ?? 100 };
}

const STD_OPTS = { aspectLock: true, centerAnchor: false, minSize: 4 } as const;

describe("multiResize — proportional scaling", () => {
  it("2 members side-by-side, SE handle, aspectLock: both scale proportionally and the gap scales too", () => {
    // Two equal-width boxes with a 50px gap between them. Total union is
    // (0,0,250,100). Dragging the SE handle to world (500,200) should double
    // every dimension (scale=2), preserving the 50px gap (becomes 100px) and
    // every member's relative size.
    const a = member({ id: "a", x: 0, y: 0, w: 100, h: 100 });
    const b = member({ id: "b", x: 150, y: 0, w: 100, h: 100 });
    const u = union({ x: 0, y: 0, w: 250, h: 100 });

    const result = multiResize([a, b], u, "se", { x: 500, y: 200 }, STD_OPTS);

    // Union exactly doubles: width 250->500, height 100->200.
    expect(result.union.w).toBeCloseTo(500, 6);
    expect(result.union.h).toBeCloseTo(200, 6);
    expect(result.union.x).toBeCloseTo(0, 6);
    expect(result.union.y).toBeCloseTo(0, 6);

    // Member A: origin (0,0) -> (0,0); size (100,100) -> (200,200).
    const a2 = result.members.find((m) => m.id === "a")!;
    expect(a2.x).toBeCloseTo(0, 6);
    expect(a2.y).toBeCloseTo(0, 6);
    expect(a2.w).toBeCloseTo(200, 6);
    expect(a2.h).toBeCloseTo(200, 6);

    // Member B: origin (150,0) -> (300,0) (anchor is union TL = (0,0), so
    // 150*2=300); size (100,100) -> (200,200). The 50px gap between A's right
    // edge (200) and B's left edge (300) doubled to 100.
    const b2 = result.members.find((m) => m.id === "b")!;
    expect(b2.x).toBeCloseTo(300, 6);
    expect(b2.y).toBeCloseTo(0, 6);
    expect(b2.w).toBeCloseTo(200, 6);
    expect(b2.h).toBeCloseTo(200, 6);

    // Relative geometry preserved: ratios of sizes, positions, and gaps are
    // constant across the resize.
    const origGap = b.x - (a.x + a.w); // 50
    const newGap = b2.x - (a2.x + a2.w);
    expect(newGap / origGap).toBeCloseTo(2, 6);
    expect(b2.w / a.w).toBeCloseTo(2, 6);
    expect(b2.x / b.x).toBeCloseTo(2, 6);
  });

  it("centerAnchor (Alt): union center fixed, both members scale away from it", () => {
    // Same pair, but anchor is the union center (125, 50). Dragging the SE
    // handle to world (500, 200): pointer delta from center is (375, 150),
    // original half-union is (125, 50), so sx = sy = 3. New union: width =
    // 250*3 = 750, centered at 125, so x = 125 - 750/2 = -250.
    const a = member({ id: "a", x: 0, y: 0, w: 100, h: 100 });
    const b = member({ id: "b", x: 150, y: 0, w: 100, h: 100 });
    const u = union({ x: 0, y: 0, w: 250, h: 100 });

    const result = multiResize(
      [a, b],
      u,
      "se",
      { x: 500, y: 200 },
      {
        ...STD_OPTS,
        centerAnchor: true,
      },
    );

    expect(result.union.w).toBeCloseTo(750, 6);
    expect(result.union.h).toBeCloseTo(300, 6);
    expect(result.union.x).toBeCloseTo(-250, 6);
    expect(result.union.y).toBeCloseTo(-100, 6);

    // Member A: distance from center (125, 50) is (-125, -50); scaled by 3
    // gives (-375, -150), so new position = (125 - 375, 50 - 150) = (-250, -100).
    const a2 = result.members.find((m) => m.id === "a")!;
    expect(a2.x).toBeCloseTo(-250, 6);
    expect(a2.y).toBeCloseTo(-100, 6);
    expect(a2.w).toBeCloseTo(300, 6);
    expect(a2.h).toBeCloseTo(300, 6);
  });

  it("aspectLock OFF (Shift): independent axes", () => {
    // Drag the SE handle to (500, 200) on a 250x100 union: independent scales
    // are sx = 500/250 = 2 and sy = 200/100 = 2 (equal here by accident);
    // try an asymmetric pointer to expose the difference.
    const a = member({ id: "a", x: 0, y: 0, w: 100, h: 100 });
    const b = member({ id: "b", x: 150, y: 0, w: 100, h: 100 });
    const u = union({ x: 0, y: 0, w: 250, h: 100 });

    // Drag to (500, 150): sx = 500/250 = 2, sy = 150/100 = 1.5 — the Y scale
    // would be forced to 2 under aspect lock (dominant axis), but stays at
    // 1.5 with aspect-lock OFF.
    const result = multiResize(
      [a, b],
      u,
      "se",
      { x: 500, y: 150 },
      {
        aspectLock: false,
        centerAnchor: false,
        minSize: 4,
      },
    );

    expect(result.union.w).toBeCloseTo(500, 6);
    expect(result.union.h).toBeCloseTo(150, 6);

    const a2 = result.members.find((m) => m.id === "a")!;
    expect(a2.w).toBeCloseTo(200, 6);
    expect(a2.h).toBeCloseTo(150, 6);

    const b2 = result.members.find((m) => m.id === "b")!;
    expect(b2.x).toBeCloseTo(300, 6);
    expect(b2.y).toBeCloseTo(0, 6);
    expect(b2.w).toBeCloseTo(200, 6);
    expect(b2.h).toBeCloseTo(150, 6);
  });

  it("derived member: position scales, size untouched", () => {
    // Text-like member at (50,0,40,20). AspectLock ON, SE handle to (400,200)
    // on union (0,0,250,100): scale = max(400/250, 200/100) = 2.
    const a = member({ id: "a", x: 0, y: 0, w: 100, h: 100 });
    const txt = member({
      id: "txt",
      x: 50,
      y: 50,
      w: 40,
      h: 20,
      derivedSize: true,
    });
    const u = union({ x: 0, y: 0, w: 250, h: 100 });

    const result = multiResize([a, txt], u, "se", { x: 400, y: 200 }, STD_OPTS);

    const txt2 = result.members.find((m) => m.id === "txt")!;
    // Position scales (anchor=(0,0) here): (50,50) -> (100,100).
    expect(txt2.x).toBeCloseTo(100, 6);
    expect(txt2.y).toBeCloseTo(100, 6);
    // Size UNCHANGED — caller will re-derive after commit.
    expect(txt2.w).toBeCloseTo(40, 6);
    expect(txt2.h).toBeCloseTo(20, 6);
    expect(txt2.derivedSize).toBe(true);
  });

  it("minSize clamp binds on the smallest member → uniform scale stops there", () => {
    // Two members. A is 100x100; B is 10x10. Union is (0,0,250,100). Anchor
    // for SE handle = (0, 0). Pointer at (12.5, 5) gives sx = 12.5/250 = 0.05
    // and sy = 5/100 = 0.05. With minSize=4, member B's required scale floor
    // is min(4/10, 4/10) = 0.4 on BOTH axes — so the proposed 0.05 would
    // shrink B to 0.5, way below the floor. Aspect-lock forces uniform scale,
    // so the whole set clamps UP to B's limit (0.4). A ends at 100*0.4 = 40,
    // B ends at 10*0.4 = 4 (the exact minSize).
    const a = member({ id: "a", x: 0, y: 0, w: 100, h: 100 });
    const b = member({ id: "b", x: 150, y: 0, w: 10, h: 10 });
    const u = union({ x: 0, y: 0, w: 250, h: 100 });

    const result = multiResize([a, b], u, "se", { x: 12.5, y: 5 }, STD_OPTS);

    // The clamp bound sx=sy=0.4 (the size B can bear). No member was distorted
    // — A's w/h scaled by 0.4, B's w/h scaled by 0.4 to exactly minSize.
    const a2 = result.members.find((m) => m.id === "a")!;
    const b2 = result.members.find((m) => m.id === "b")!;
    expect(a2.w).toBeCloseTo(40, 6);
    expect(a2.h).toBeCloseTo(40, 6);
    expect(b2.w).toBeCloseTo(4, 6);
    expect(b2.h).toBeCloseTo(4, 6);
    // Union stopped at 0.4x — proves the scale was raised uniformly.
    expect(result.union.w).toBeCloseTo(100, 6);
    expect(result.union.h).toBeCloseTo(40, 6);
  });

  it("drag past the anchor → clamped to minSize, never negative", () => {
    // NW handle on union (100,100,200,200). Anchor (without center) for NW
    // is the opposite corner = (300, 300). Dragging the pointer to (50, 50)
    // puts the pointer on the wrong side of the anchor on both axes (raw
    // scales go negative). Per the v1 spec we REJECT the negative (sx = sy =
    // 0), then the minSize clamp raises scale just enough to land the member
    // at exactly minSize — the box collapses to a minSize square around the
    // anchor. No negative dimensions, no zero-area collapse: the box always
    // has a positive footprint.
    const a = member({ id: "a", x: 100, y: 100, w: 200, h: 200 });
    const u = union({ x: 100, y: 100, w: 200, h: 200 });

    const result = multiResize([a], u, "nw", { x: 50, y: 50 }, STD_OPTS);

    const a2 = result.members.find((m) => m.id === "a")!;
    // Member collapsed to minSize on both axes (no negatives).
    expect(a2.w).toBeCloseTo(4, 6);
    expect(a2.h).toBeCloseTo(4, 6);
    // Box footprint still positive — union has a non-zero area.
    expect(result.union.w).toBeCloseTo(4, 6);
    expect(result.union.h).toBeCloseTo(4, 6);
  });

  it("no members / zero-sized union → identity", () => {
    const r = multiResize([], union(), "se", { x: 100, y: 100 }, STD_OPTS);
    expect(r.members).toEqual([]);
    expect(r.union).toEqual(union());
  });

  // v4-pro P4 review finding #2: without aspect lock the minSize floors must
  // be PER-AXIS — a tall-thin member's binding WIDTH floor must not make the
  // perpendicular (vertical) axis sticky.
  it("non-aspect-lock minSize floors are independent per axis", () => {
    const thin = member({ id: "thin", x: 0, y: 0, w: 10, h: 100 });
    const wide = member({ id: "wide", x: 20, y: 0, w: 100, h: 100 });
    const u = union({ x: 0, y: 0, w: 120, h: 100 });

    // se handle, anchor (0,0). Pointer (48, 5): requested sx=0.4, sy=0.05.
    // Floors with minSize 4: floorX = 4/10 = 0.4 (binds exactly),
    // floorY = 4/100 = 0.04 — sy must stay 0.05, NOT be raised to 0.4.
    const r = multiResize(
      [thin, wide],
      u,
      "se",
      { x: 48, y: 5 },
      {
        aspectLock: false,
        centerAnchor: false,
        minSize: 4,
      },
    );

    const rThin = r.members.find((m) => m.id === "thin");
    if (!rThin) throw new Error("thin member missing");
    expect(rThin.w).toBeCloseTo(4, 6); // width floor binds
    expect(rThin.h).toBeCloseTo(5, 6); // height scales freely (old bug: 40)
  });

  it("SW handle: anchor is top-right of the union", () => {
    // Union (0,0,200,100). SW anchor without center = (200, 0). Drag pointer
    // to (100, 200): sx = (100-200)/200 = -0.5, sy = (200-0)/100 = 2. After
    // rejection on negative sx, sx = 0; sy = 2. With aspect lock, both go to
    // max(0, 2) = 2. New union: width = 200*2 = 400 (still anchored at x=200,
    // so x = 200-400 = -200), height = 100*2 = 200, y = 0.
    const a = member({ id: "a", x: 0, y: 0, w: 200, h: 100 });
    const u = union({ x: 0, y: 0, w: 200, h: 100 });

    const result = multiResize([a], u, "sw", { x: 100, y: 200 }, STD_OPTS);
    expect(result.union.x).toBeCloseTo(-200, 6);
    expect(result.union.w).toBeCloseTo(400, 6);
    expect(result.union.y).toBeCloseTo(0, 6);
    expect(result.union.h).toBeCloseTo(200, 6);
  });
});
