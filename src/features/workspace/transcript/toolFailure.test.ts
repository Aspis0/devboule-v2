import { describe, expect, it } from "vitest";
import { isFailedToolCall } from "./toolFailure";

describe("isFailedToolCall", () => {
  it("counts a call the provider reports failed when it has no exit code", () => {
    expect(isFailedToolCall({ status: "failed" })).toBe(true);
    expect(isFailedToolCall({ status: "Failed" })).toBe(true);
  });

  it("counts a provider-reported failure whose exit code is zero", () => {
    expect(isFailedToolCall({ status: "failed", exitCode: 0 })).toBe(true);
  });

  it("keeps a provider-reported failure failed whatever its exit code", () => {
    expect(isFailedToolCall({ status: "failed", exitCode: 1 })).toBe(true);
    expect(isFailedToolCall({ status: "failed", exitCode: 2 })).toBe(true);
  });

  it("does not count a non-zero exit alone as a failure (git diff, grep)", () => {
    expect(isFailedToolCall({ status: "completed", exitCode: 1 })).toBe(false);
    expect(isFailedToolCall({ status: "completed", exitCode: 2 })).toBe(false);
  });

  it("does not count a call that is not failed", () => {
    expect(isFailedToolCall({ status: "completed" })).toBe(false);
    expect(isFailedToolCall({ status: "running", exitCode: 0 })).toBe(false);
  });
});
