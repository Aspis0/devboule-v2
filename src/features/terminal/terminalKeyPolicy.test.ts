import { describe, expect, it, vi } from "vitest";
import { terminalKeyPolicy } from "./terminalKeyPolicy";

describe("terminalKeyPolicy", () => {
  it("passes ordinary keys through", () => {
    const onCtrlC = vi.fn();
    expect(terminalKeyPolicy({ type: "keydown", ctrlKey: false, key: "a" }, onCtrlC)).toBe(true);
    expect(onCtrlC).not.toHaveBeenCalled();
  });

  it("swallows plain Ctrl+C and calls back only on keydown", () => {
    const onCtrlC = vi.fn();
    expect(terminalKeyPolicy({ type: "keydown", ctrlKey: true, key: "c" }, onCtrlC)).toBe(false);
    expect(terminalKeyPolicy({ type: "keyup", ctrlKey: true, key: "c" }, onCtrlC)).toBe(false);
    expect(onCtrlC).toHaveBeenCalledTimes(1);
  });

  it("passes copy and AltGr variants through", () => {
    const onCtrlC = vi.fn();
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: true, shiftKey: true, key: "C" }, onCtrlC),
    ).toBe(true);
    expect(
      terminalKeyPolicy({ type: "keydown", ctrlKey: true, altKey: true, key: "c" }, onCtrlC),
    ).toBe(true);
    expect(terminalKeyPolicy({ type: "keydown", ctrlKey: false, key: "c" }, onCtrlC)).toBe(true);
    expect(onCtrlC).not.toHaveBeenCalled();
  });
});
