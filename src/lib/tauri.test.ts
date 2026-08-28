import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { invokeTyped, isCommandError } from "./tauri";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class Channel {},
}));

describe("isCommandError", () => {
  it("recognizes the JSON object Tauri rejects a Serialize error as", () => {
    const payload = {
      code: "session_not_found",
      message: "No session with that id.",
    };
    expect(isCommandError(payload)).toBe(true);
    if (isCommandError(payload)) {
      expect(payload.code).toBe("session_not_found");
      expect(payload.message).toBe("No session with that id.");
    }
  });

  it("recognizes an error with details", () => {
    const payload = {
      code: "session_generation_mismatch",
      message: "session generation is 3, client cursor is 1",
      details: { type: "generation_mismatch", current: 3, requested: 1 },
    };
    expect(isCommandError(payload)).toBe(true);
  });

  it("rejects strings and Error instances that old commands used to throw", () => {
    expect(isCommandError("No session with that id.")).toBe(false);
    expect(isCommandError(new Error("No session with that id."))).toBe(false);
    expect(isCommandError(null)).toBe(false);
    expect(isCommandError({ message: "missing code" })).toBe(false);
  });
});

describe("invokeTyped rejected payload", () => {
  it("surfaces a structured command error, not a string", async () => {
    const payload = {
      code: "session_not_found",
      message: "No session with that id.",
    };
    vi.mocked(invoke).mockRejectedValueOnce(payload);

    try {
      await invokeTyped("session_detach", { id: "missing" });
      throw new Error("expected invokeTyped to reject");
    } catch (error) {
      expect(error).toBe(payload);
      expect(isCommandError(error)).toBe(true);
      if (!isCommandError(error)) return;
      expect(error.code).toBe("session_not_found");
      expect(error.message).toBe("No session with that id.");
    }
  });
});
