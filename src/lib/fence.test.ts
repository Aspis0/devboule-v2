import { describe, expect, it } from "vitest";
import { isCopyableFence } from "./fence";

describe("fence classification", () => {
  it.each(["sh", "bash", "shell", "zsh", "console", "powershell", "ps1", "cmd", "env", "dotenv"])(
    "treats a %s fence as a copyable block whatever its length",
    (tag) => {
      expect(isCopyableFence(tag, "a\nb\nc\nd\ne")).toBe(true);
    },
  );

  it("treats an untagged fence of at most three lines as a copyable block", () => {
    expect(isCopyableFence(undefined, "pnpm build")).toBe(true);
    expect(isCopyableFence(undefined, "a\nb\nc")).toBe(true);
    expect(isCopyableFence("", "a\nb\nc")).toBe(true);
    expect(isCopyableFence("   ", "a\nb")).toBe(true);
  });

  it("counts content lines only, not a blank line left before the closing fence", () => {
    expect(isCopyableFence(undefined, "a\nb\nc\n\n")).toBe(true);
    expect(isCopyableFence(undefined, "a\nb\nc\nd")).toBe(false);
    expect(isCopyableFence(undefined, "a\nb\nc\nd\n\n")).toBe(false);
  });

  it("treats a fence tagged with anything else as a code sample", () => {
    expect(isCopyableFence("ts", "a")).toBe(false);
    expect(isCopyableFence("rust", "a\nb")).toBe(false);
    expect(isCopyableFence("python", "a\nb\nc")).toBe(false);
    expect(isCopyableFence("env.local", "a")).toBe(false);
  });

  it("matches the tag case-insensitively and ignores what follows it", () => {
    expect(isCopyableFence("BASH", "a\nb\nc\nd\ne")).toBe(true);
    expect(isCopyableFence("bash -x", "a\nb\nc\nd\ne")).toBe(true);
    expect(isCopyableFence("  Zsh  ", "a\nb\nc\nd\ne")).toBe(true);
  });
});
