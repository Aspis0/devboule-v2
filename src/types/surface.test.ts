import { describe, expect, it } from "vitest";
import { SURFACES } from "./surface";

const topBar = SURFACES.filter((surface) => surface.inTopBar !== false).map(
  (surface) => surface.key,
);

describe("the top bar's surfaces", () => {
  it("lists the four core surfaces and nothing else", () => {
    expect(topBar).toEqual(["workspace", "design", "settings", "marketplace"]);
  });

  it("keeps Pubvia out of the app altogether", () => {
    expect(SURFACES.map((surface) => surface.key)).not.toContain("pubvia");
  });

  it("keeps Polis defined as a plugin surface that the top bar does not list", () => {
    const polis = SURFACES.find((surface) => surface.key === "polis");
    expect(polis?.plugin).toBe("polis");
    expect(polis?.inTopBar).toBe(false);
    expect(topBar).not.toContain("polis");
  });
});
