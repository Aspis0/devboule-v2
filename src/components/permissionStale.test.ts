import { describe, expect, it } from "vitest";
import { STALE_PERMISSION_LINE, isStalePermissionError } from "./permissionStale";

describe("isStalePermissionError", () => {
  it("treats the broker's no-longer-pending refusal as stale", () => {
    expect(
      isStalePermissionError({
        code: "invalid_request",
        message: "permission request is no longer pending",
      }),
    ).toBe(true);
  });

  it("treats the missing-broker refusal as stale", () => {
    expect(
      isStalePermissionError({
        code: "invalid_request",
        message: "Session has no live ACP permission broker.",
      }),
    ).toBe(true);
  });

  it("does not treat a bad option id as stale, though it shares the code", () => {
    // select_option's three refusals all ride `invalid_request` too, so the
    // code alone can never be the discriminator.
    expect(
      isStalePermissionError({
        code: "invalid_request",
        message: "Unknown permission option 'nope'.",
      }),
    ).toBe(false);
    expect(
      isStalePermissionError({
        code: "invalid_request",
        message: "Permission option 'allow' cannot be used for Deny.",
      }),
    ).toBe(false);
    expect(
      isStalePermissionError({
        code: "invalid_request",
        message:
          "Permission request offers no 'allow_once' option (offered: allow_always); the request stays pending",
      }),
    ).toBe(false);
  });

  it("treats the session being gone as stale", () => {
    // The Tauri boundary rewrites SessionNotFound to this shape
    // (src-tauri/src/backend/error.rs:31-40): no session, no broker, no
    // answer that could ever succeed.
    expect(
      isStalePermissionError({ code: "session_not_found", message: "No session with that id." }),
    ).toBe(true);
  });

  it("matches the stale sentences case-insensitively", () => {
    expect(
      isStalePermissionError({
        code: "invalid_request",
        message: "Permission Request Is No Longer Pending",
      }),
    ).toBe(true);
  });

  it("needs both halves of the missing-broker sentence", () => {
    expect(
      isStalePermissionError({ code: "invalid_request", message: "has no live subscription" }),
    ).toBe(false);
    expect(
      isStalePermissionError({ code: "invalid_request", message: "permission broker is closed" }),
    ).toBe(false);
  });

  it("does not treat an unrelated invalid request as stale", () => {
    expect(
      isStalePermissionError({
        code: "invalid_request",
        message: "Permission request id is required.",
      }),
    ).toBe(false);
  });
  it("does not treat other codes as stale", () => {
    expect(isStalePermissionError({ code: "internal", message: "boom" })).toBe(false);
    expect(isStalePermissionError({ code: "unauthorized", message: "gone" })).toBe(false);
  });

  it("never calls an app-authored failure stale — nothing provable on it", () => {
    expect(isStalePermissionError(new Error("live failed"))).toBe(false);
    expect(isStalePermissionError("permission request is no longer pending")).toBe(false);
    expect(isStalePermissionError(null)).toBe(false);
    expect(isStalePermissionError(undefined)).toBe(false);
    expect(isStalePermissionError({ code: "invalid_request" })).toBe(false);
  });
});

describe("STALE_PERMISSION_LINE", () => {
  it("is the one short terminal sentence", () => {
    expect(STALE_PERMISSION_LINE).toBe("This request is no longer pending.");
  });
});
