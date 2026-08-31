import { describe, expect, it } from "vitest";
import { MAX_ZOOM } from "./camera";
import { CITIZEN_BAKE_RESOLUTION } from "./citizenAtlas";

describe("interactive zoom ceiling", () => {
  it("allows citizen inspection and bakes citizen art at that ceiling", () => {
    expect(MAX_ZOOM).toBe(6);
    expect(CITIZEN_BAKE_RESOLUTION).toBe(MAX_ZOOM);
  });
});
