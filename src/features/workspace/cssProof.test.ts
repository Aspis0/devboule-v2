// assembleCssProof skips a natively nested rule instead of throwing: the
// proof layer asserts selected DOM states, and a nested rule has no flat
// form — while the type-floor walk still fails loudly on the same input.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { assembleCssProof } from "./cssProof";

describe("assembleCssProof on native nesting", () => {
  it("skips the nested rule and keeps the rules around it", () => {
    const proof = assembleCssProof([
      ".outer { color: red; .inner { font-size: 9px; } } .plain { color: blue; }",
    ]);
    expect(proof.rulesFor(".outer")).toBe("");
    expect(proof.rulesFor(".inner")).toBe("");
    expect(proof.rulesFor(".plain")).toContain("color: blue;");
  });

  it("skips a nested rule inside a conditional at-rule", () => {
    const proof = assembleCssProof([
      "@media (max-width: 900px) { .outer { color: red; .inner { font-size: 9px; } } } .plain { color: blue; }",
    ]);
    expect(proof.rulesFor(".outer")).toBe("");
    expect(proof.rulesFor(".plain")).toContain("color: blue;");
  });
});
