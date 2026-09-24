import { describe, expect, it } from "vitest";
import { firstGrapheme } from "./graphemeBound";

describe("firstGrapheme", () => {
  it("returns exactly one grapheme and never an ellipsis", () => {
    for (const name of ["devboule", "api-server", "project-9"]) {
      const letter = firstGrapheme(name);
      expect(letter).toHaveLength(1);
      expect(letter).not.toContain("…");
    }
  });

  it("keeps a decomposed letter whole (base + combining marks)", () => {
    const decomposed = "e\u0301cole"; // é as e + U+0301
    expect(firstGrapheme(decomposed)).toBe("e\u0301");
  });

  it("keeps an emoji cluster whole", () => {
    expect(firstGrapheme("👨‍👩‍👧 family")).toBe("👨‍👩‍👧");
  });

  it("trims leading whitespace and answers empty for empty input", () => {
    expect(firstGrapheme("  beta")).toBe("b");
    expect(firstGrapheme("")).toBe("");
  });
});
