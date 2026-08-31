import { describe, expect, it } from "vitest";
import {
  CITIZEN_PHASE_STEPS,
  CitizenTextureAtlas,
  citizenTypeForProvider,
  citizenVariantKey,
  drawParamsForCitizen,
  type CitizenPhaseStep,
} from "./citizenAtlas";
import { BuildingTextureAtlas } from "./buildingAtlas";

describe("procedural citizen atlas", () => {
  it("maps the five providers to five structural citizen silhouettes", () => {
    const providers = ["claude", "codex", "grok", "pi", "copilot"];
    const types = providers.map(citizenTypeForProvider);

    expect(types).toEqual(["noble", "builder", "foreigner", "watercarrier", "priest"]);
    expect(new Set(types).size).toBe(providers.length);
    expect(citizenTypeForProvider("unlisted-provider")).toBe("citizen");
  });

  it("keeps state mapping in the v1 drawCitizen contract", () => {
    const working = drawParamsForCitizen("working", 3);
    expect(working.moving).toBe(true);
    expect(working.phase).not.toBe(0);
    expect(working.actionPhase).not.toBe(0);

    for (const state of ["silent", "finished", "idle"] as const) {
      const params = drawParamsForCitizen(state, 3);
      expect(params.moving).toBe(false);
      expect(params.actionPhase).toBe(0);
    }
  });

  it("uses a fixed stepped key and captures each variant once", () => {
    expect(CITIZEN_PHASE_STEPS).toBe(8);
    expect(citizenVariantKey("builder", "working", 7)).toBe("citizen:builder:working:s7");

    let captures = 0;
    const renderer = {
      generateTexture: () => {
        captures += 1;
        return {} as never;
      },
    };
    const owner = new BuildingTextureAtlas(1);
    const atlas = new CitizenTextureAtlas(owner);
    const step = 2 as CitizenPhaseStep;
    const first = atlas.get(renderer, "builder", "working", step);
    const second = atlas.get(renderer, "builder", "working", step);

    expect(first.texture).toBe(second.texture);
    expect(captures).toBe(1);
    expect(atlas.size).toBe(1);
  });
});
