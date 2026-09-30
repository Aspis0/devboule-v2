import { describe, expect, it } from "vitest";
import { providerEmptySentence } from "./providerEmptySentence";

describe("providerEmptySentence", () => {
  it("names the unreadable PATH directories instead of asserting their absence", () => {
    expect(providerEmptySentence(2)).toBe(
      "No agent CLI found, but 2 PATH directories could not be read",
    );
  });

  it("states the plain absence when every PATH directory was readable", () => {
    expect(providerEmptySentence(0)).toBe("No agent CLI found on PATH");
  });
});
