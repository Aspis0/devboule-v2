import { describe, expect, it } from "vitest";
import { currentTurnIndex } from "./currentTurn";

interface AnchorProbe {
  read: (index: number) => number;
  readonly state: { reads: number };
}

function readerFor(tops: readonly number[]): AnchorProbe {
  const state = { reads: 0 };
  return {
    state,
    read: (index: number) => {
      state.reads += 1;
      return tops[index];
    },
  };
}

describe("currentTurnIndex", () => {
  it("has no current turn without anchors", () => {
    expect(currentTurnIndex(() => 0, 0, 0)).toBe(-1);
  });

  it("returns no index when no bubble is above the top, for the caller's first-turn fallback", () => {
    const { read, state } = readerFor([100, 400, 700]);
    expect(currentTurnIndex(read, 3, 0)).toBe(-1);
    expect(state.reads).toBeGreaterThan(0);
  });

  it("takes the last bubble at or above the top", () => {
    // Tops in document order: with the viewport top at 450, the bubbles at
    // 100 and 400 are at or above it, the one at 700 is not.
    expect(currentTurnIndex(readerFor([100, 400, 700]).read, 3, 450)).toBe(1);
    expect(currentTurnIndex(readerFor([100, 400, 700]).read, 3, 150)).toBe(0);
    expect(currentTurnIndex(readerFor([100, 400, 700]).read, 3, 900)).toBe(2);
  });

  it("counts a scroll landing a hair below the top edge as at it, but not a visible gap", () => {
    // Fractional scroll positions: a scrollport that settles at viewTop
    // + 0.5 still counts that bubble as the turn at the top.
    expect(currentTurnIndex(readerFor([0.5, 400]).read, 2, 0)).toBe(0);
    expect(currentTurnIndex(readerFor([2, 400]).read, 2, 0)).toBe(-1);
  });

  it("reads a logarithmic budget of tops per frame, not one per turn", () => {
    const tops = Array.from({ length: 300 }, (_, index) => index * 200);
    const { read, state } = readerFor(tops);
    expect(currentTurnIndex(read, 300, 150_000)).toBe(299);
    // ceil(log2(301)) — nine tops at 300 turns, whatever the frame asks.
    expect(state.reads).toBeLessThanOrEqual(9);
  });

  it("costs well under a frame per decision at 300 turns", () => {
    const tops = Array.from({ length: 300 }, (_, index) => index * 200);
    const { read, state } = readerFor(tops);
    const iterations = 20_000;
    let result = -1;
    const started = performance.now();
    for (let i = 0; i < iterations; i += 1) {
      state.reads = 0;
      result = currentTurnIndex(read, 300, 150_000 + (i % 13));
    }
    const perDecisionUs = ((performance.now() - started) / iterations) * 1000;
    expect(result).toBe(299);
    // A scroll frame has 16 ms; this rule's whole share of it is this.
    expect(perDecisionUs).toBeLessThan(100);
    console.log(`currentTurnIndex at 300 turns: ${perDecisionUs.toFixed(1)} µs/decision`);
  });
});
