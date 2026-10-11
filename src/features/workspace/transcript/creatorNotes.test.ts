import { describe, expect, it } from "vitest";
import { creationFailureReason, isCreationStartNote } from "./creatorNotes";

describe("creator notes", () => {
  it("reads a start as traffic, and a failure as its reason", () => {
    expect(isCreationStartNote("The person approved the agent creation: 'Kid' started.")).toBe(
      true,
    );
    expect(isCreationStartNote("The agent creation did not start: no provider.")).toBe(false);
    expect(creationFailureReason("The agent creation did not start: no provider.")).toBe(
      "no provider.",
    );
    expect(creationFailureReason("The person approved the agent creation: 'Kid'.")).toBeNull();
  });
});
