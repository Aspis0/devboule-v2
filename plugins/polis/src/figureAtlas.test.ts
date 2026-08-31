import { describe, expect, it } from "vitest";
import {
  figureVariantKey,
  providerFigureKind,
  statePoseKind,
  type FigurePose,
  type FigureProvider,
  type FigureState,
} from "./figureAtlas";

const providers: FigureProvider[] = ["claude", "codex", "grok", "pi", "copilot"];
const states: FigureState[] = ["working", "silent", "finished", "idle"];

describe("Greek figure variants", () => {
  it("gives every provider a distinct silhouette family", () => {
    const families = providers.map((provider) => providerFigureKind(provider));
    expect(new Set(families).size).toBe(providers.length);
  });

  it("gives each live state a distinct pose family", () => {
    const poses = states.map((state) => statePoseKind(state));
    expect(new Set(poses).size).toBe(states.length);
    expect(statePoseKind("silent")).not.toBe(statePoseKind("finished"));
  });

  it("keys the baked art by provider, state, and pose", () => {
    const key = figureVariantKey("pi", "silent", 1 as FigurePose);
    expect(key).toBe("pi:silent:p1");
    expect(key).not.toBe(figureVariantKey("pi", "finished", 1));
    expect(key).not.toBe(figureVariantKey("codex", "silent", 1));
  });
});
