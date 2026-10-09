import { describe, expect, it } from "vitest";
import { modelLabel } from "./modelLabel";

describe("modelLabel", () => {
  it("names the provider before the model it serves", () => {
    expect(modelLabel({ name: "GLM-5.3 Flash", providerId: "opencode-go" })).toBe(
      "opencode-go · GLM-5.3 Flash",
    );
  });

  it("keeps the same model name apart when two providers serve it", () => {
    expect(
      modelLabel({ name: "NVIDIA: Nemotron 3 Ultra (free)", providerId: "openrouter" }),
    ).not.toBe(modelLabel({ name: "NVIDIA: Nemotron 3 Ultra (free)", providerId: "opencode-go" }));
  });

  it("is the bare name when no provider was reported", () => {
    expect(modelLabel({ name: "Opus 5" })).toBe("Opus 5");
    expect(modelLabel({ name: "Opus 5", providerId: "" })).toBe("Opus 5");
  });
});
