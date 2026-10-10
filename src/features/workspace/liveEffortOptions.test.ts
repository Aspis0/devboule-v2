import { describe, expect, it } from "vitest";
import { liveEffortOptions } from "./liveEffortOptions";

const EFFORTS = [
  { id: "low", label: "Low" },
  { id: "high", label: "High" },
  { id: "max", label: "Max" },
];

describe("the live effort picker", () => {
  it("offers no max to a live Claude session that is not already on max", () => {
    expect(liveEffortOptions(EFFORTS, "claude", "high").map((effort) => effort.id)).toEqual([
      "low",
      "high",
    ]);
  });

  it("keeps max on the list when it is the level the session already has", () => {
    expect(liveEffortOptions(EFFORTS, "claude", "max").map((effort) => effort.id)).toEqual([
      "low",
      "high",
      "max",
    ]);
  });

  it("leaves other providers' lists whole, max included", () => {
    expect(liveEffortOptions(EFFORTS, "codex", "high")).toHaveLength(3);
    expect(liveEffortOptions(EFFORTS, undefined, undefined)).toHaveLength(3);
  });
});
