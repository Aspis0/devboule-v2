import { afterEach, describe, expect, it, vi } from "vitest";
import { waitAtMost } from "./waitAtMost";

afterEach(() => {
  vi.useRealTimers();
});

describe("waitAtMost", () => {
  it("resolves as soon as the pending call settles, before the deadline", async () => {
    vi.useFakeTimers();
    let settled = false;
    const done = waitAtMost(2_000, Promise.resolve("ok")).then(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(0);
    await done;
    expect(settled).toBe(true);
  });

  it("resolves at the deadline when the pending call never settles", async () => {
    vi.useFakeTimers();
    let settled = false;
    const done = waitAtMost(2_000, new Promise(() => undefined)).then(() => {
      settled = true;
    });

    await vi.advanceTimersByTimeAsync(1_999);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    await done;
    expect(settled).toBe(true);
  });

  it("resolves when the pending call fails, so the caller reads what is stored", async () => {
    await expect(waitAtMost(2_000, Promise.reject(new Error("journal busy")))).resolves.toBe(
      undefined,
    );
  });
});
