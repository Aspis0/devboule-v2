import { describe, expect, it } from "vitest";
import { CLOSE_BEHAVIOR_SURFACE_ID, closeChoiceFromStored } from "./closeBehaviorChoice";

describe("closeChoiceFromStored", () => {
  it("reads each stored choice", () => {
    expect(closeChoiceFromStored({ choice: "ask" })).toBe("ask");
    expect(closeChoiceFromStored({ choice: "tray" })).toBe("tray");
    expect(closeChoiceFromStored({ choice: "quit" })).toBe("quit");
  });

  it("falls back to ask for anything it does not recognize", () => {
    // An unreadable or future choice must never skip the confirmation:
    // asking is the only direction that cannot stop a daemon silently.
    expect(closeChoiceFromStored(undefined)).toBe("ask");
    expect(closeChoiceFromStored(null)).toBe("ask");
    expect(closeChoiceFromStored({})).toBe("ask");
    expect(closeChoiceFromStored({ choice: "minimize" })).toBe("ask");
    expect(closeChoiceFromStored({ choice: 7 })).toBe("ask");
    expect(closeChoiceFromStored("quit")).toBe("ask");
  });

  it("uses a surface id the backend accepts as a filename", () => {
    expect(CLOSE_BEHAVIOR_SURFACE_ID).toMatch(/^[a-z0-9-]{1,32}$/);
  });
});
