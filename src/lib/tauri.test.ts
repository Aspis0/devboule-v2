import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  invokeTyped,
  isCommandError,
  journalRetentionGet,
  journalRetentionSet,
  journalUsage,
  providersRefresh,
  sessionDelete,
  sessionResume,
  surfaceSettingsGet,
  surfaceSettingsSet,
} from "./tauri";

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

describe("retention command wrappers", () => {
  it("calls all four registered retention commands with typed payloads", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({} as never);
    await journalUsage();
    await journalRetentionGet();
    await journalRetentionSet({ maxAgeMs: 0 });
    await sessionDelete("s.owner.1");

    expect(invoke).toHaveBeenNthCalledWith(1, "journal_usage", undefined);
    expect(invoke).toHaveBeenNthCalledWith(2, "journal_retention_get", undefined);
    expect(invoke).toHaveBeenNthCalledWith(3, "journal_retention_set", {
      maxAgeMs: 0,
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "session_delete", { id: "s.owner.1" });
  });
});

describe("surface settings wrappers", () => {
  it("passes the surfaceId and opaque value payloads through", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(null as never);
    await surfaceSettingsGet("design");
    await surfaceSettingsSet("design", { split: true, count: 3 });

    expect(invoke).toHaveBeenNthCalledWith(1, "surface_settings_get", {
      surfaceId: "design",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "surface_settings_set", {
      surfaceId: "design",
      value: { split: true, count: 3 },
    });
  });
});

describe("resume command wrapper", () => {
  it("passes the camelCase sessionId expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ type: "not_supported" } as never);
    await sessionResume("s.owner.1");

    expect(invoke).toHaveBeenCalledWith("session_resume", { sessionId: "s.owner.1" });
  });
});

describe("provider refresh wrapper", () => {
  it("calls the providers_refresh command with no payload", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ providers: [], unreadableDirs: 0 } as never);
    await providersRefresh();

    expect(invoke).toHaveBeenCalledWith("providers_refresh", undefined);
  });
});
