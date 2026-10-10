import { describe, expect, it } from "vitest";
import { SURFACES } from "./surface";

describe("the top bar's surfaces", () => {
  it("lists the four core surfaces and nothing else", () => {
    expect(SURFACES.map((surface) => surface.key)).toEqual([
      "workspace",
      "design",
      "settings",
      "marketplace",
    ]);
  });

  it("keeps Pubvia and Polis out of the top bar", () => {
    const keys: string[] = SURFACES.map((surface) => surface.key);
    expect(keys).not.toContain("pubvia");
    expect(keys).not.toContain("polis");
  });
});
