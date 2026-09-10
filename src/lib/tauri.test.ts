import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Channel } from "@tauri-apps/api/core";
import {
  COMMAND_ARG_KEYS,
  invokeTyped,
  isCommandError,
  journalRetentionGet,
  journalRetentionSet,
  journalUsage,
  providersRefresh,
  sessionAttach,
  sessionClaim,
  sessionDetach,
  sessionInterrupt,
  sessionPermissionRespond,
  sessionResize,
  sessionSend,
  sessionCreate,
  sessionDelete,
  sessionPresence,
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
      await invokeTyped("session_detach", { subscriptionId: 41 });
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

  it("maps a resolved null to the absent case", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(null as never);

    // The backend's Ok(None) serializes to null: genuinely absent file.
    await expect(surfaceSettingsGet("design")).resolves.toEqual({ status: "absent" });
  });

  it("maps a resolved document to the value case", async () => {
    vi.mocked(invoke).mockClear();
    const document = { version: 1, mode: "all", enabledSlugs: [] };
    vi.mocked(invoke).mockResolvedValue(document as never);

    await expect(surfaceSettingsGet("design")).resolves.toEqual({
      status: "value",
      value: document,
    });
  });

  it("maps a rejected read to the unreadable case carrying the message", async () => {
    vi.mocked(invoke).mockClear();
    // A structured CommandError is what Tauri rejects a Serialize error with.
    vi.mocked(invoke).mockRejectedValueOnce({
      code: "io_error",
      message: "settings file unreadable",
    });
    await expect(surfaceSettingsGet("design")).resolves.toEqual({
      status: "unreadable",
      message: "settings file unreadable",
    });

    // A plain Error rejection lands in the same case, message preserved.
    vi.mocked(invoke).mockRejectedValueOnce(new Error("bridge down"));
    await expect(surfaceSettingsGet("design")).resolves.toEqual({
      status: "unreadable",
      message: "bridge down",
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

describe("create and attach command wrappers", () => {
  it("sends session_create with the camelCase workspaceId expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({ id: "s.1" } as never);
    await sessionCreate(null, "terminal");

    // Tauri v2 derives JS arg names from the Rust parameters: the daemon's
    // `workspace_id: Option<String>` must be addressed as `workspaceId`. The
    // snake_case spelling was silently coerced to None before this was fixed.
    expect(invoke).toHaveBeenCalledWith("session_create", {
      workspaceId: null,
      kind: "terminal",
      provider: null,
    });
  });

  it("sends session_attach with the camelCase fromCursor expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(41 as never);
    const ch = {} as Channel;
    await expect(sessionAttach("s.owner.1", null, ch)).resolves.toBe(41);

    // Same convention: the daemon's `from_cursor: Option<u64>` is `fromCursor`
    // on the JS side.
    expect(invoke).toHaveBeenCalledWith("session_attach", {
      id: "s.owner.1",
      fromCursor: null,
      ch,
    });
  });

  it("puts the subscription id on every observer-specific command", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(undefined as never);

    await sessionSend("s.owner.1", 41, "hello");
    await sessionInterrupt("s.owner.1", 41);
    await sessionClaim(41);
    await sessionPermissionRespond("s.owner.1", 41, "tool-1", "allow_once");
    await sessionResize("s.owner.1", 41, 80, 24);
    await sessionDetach(41);

    expect(invoke).toHaveBeenNthCalledWith(1, "session_send", {
      id: "s.owner.1",
      subscriptionId: 41,
      text: "hello",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "session_interrupt", {
      id: "s.owner.1",
      subscriptionId: 41,
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "session_claim", { subscriptionId: 41 });
    expect(invoke).toHaveBeenNthCalledWith(4, "session_permission_respond", {
      id: "s.owner.1",
      subscriptionId: 41,
      requestId: "tool-1",
      outcome: "allow_once",
    });
    expect(invoke).toHaveBeenNthCalledWith(5, "session_resize", {
      id: "s.owner.1",
      subscriptionId: 41,
      cols: 80,
      rows: 24,
    });
    expect(invoke).toHaveBeenNthCalledWith(6, "session_detach", { subscriptionId: 41 });
  });
});

describe("bridge wire-key convention", () => {
  it("keeps every argument key in the command map camelCase", () => {
    const offenders = Object.entries(COMMAND_ARG_KEYS).flatMap(([command, keys]) =>
      keys.filter((key) => key.includes("_")).map((key) => `${command}: ${key}`),
    );
    // The message is carried on the assertion itself, so it shows up exactly
    // when the guard fires. Tauri v2 derives JS arg names from the Rust
    // snake_case parameters: a snake_case key is either rejected as `invalid
    // args` or silently coerced to None.
    expect(
      offenders,
      "Snake_case argument keys found in the Tauri bridge. Tauri v2 exposes Rust " +
        "snake_case parameters to JavaScript as camelCase, so a snake_case key is " +
        "either rejected as `invalid args` or silently coerced to None. Rename the " +
        "key in `CommandArgs` (src/lib/tauri.ts), in `COMMAND_ARG_KEYS`, in its " +
        "exported wrapper, and in every internal shim that builds the argument " +
        "object. Command NAMES stay snake_case; only argument keys are camelCase.\n" +
        offenders.map((line) => `  - ${line}`).join("\n"),
    ).toEqual([]);
  });
});

describe("presence command wrapper", () => {
  it("passes the camelCase keys expected by Tauri v2", async () => {
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(undefined as never);
    await sessionPresence("session-a", true);
    await sessionPresence(null, false);

    expect(invoke).toHaveBeenNthCalledWith(1, "session_presence", {
      focusedSessionId: "session-a",
      appVisible: true,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "session_presence", {
      focusedSessionId: null,
      appVisible: false,
    });
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
