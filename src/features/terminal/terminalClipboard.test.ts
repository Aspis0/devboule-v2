import { afterEach, describe, expect, it, vi } from "vitest";
import type { Terminal } from "@xterm/xterm";
import { copyTerminalSelection } from "./terminalClipboard";

function fakeTerminal(getSelection: () => string) {
  return {
    getSelection,
    clearSelection: vi.fn(),
  } as unknown as Terminal & { clearSelection: ReturnType<typeof vi.fn> };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("copyTerminalSelection", () => {
  it("writes the selection through the clipboard helper and reports success", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const terminal = fakeTerminal(() => "selected");

    expect(await copyTerminalSelection(terminal)).toBe(true);
    expect(writeText).toHaveBeenCalledWith("selected");
    expect(terminal.clearSelection).toHaveBeenCalledTimes(1);
  });

  it("reports a denied write and clears the selection anyway", async () => {
    const writeText = vi.fn().mockRejectedValue(new Error("denied"));
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const terminal = fakeTerminal(() => "selected");

    expect(await copyTerminalSelection(terminal)).toBe(false);
    // A kept selection would trap Ctrl+C on a copy that can never succeed.
    expect(terminal.clearSelection).toHaveBeenCalledTimes(1);
  });

  it("does nothing without a selection", async () => {
    const writeText = vi.fn();
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const terminal = fakeTerminal(() => "");

    expect(await copyTerminalSelection(terminal)).toBe(true);
    expect(writeText).not.toHaveBeenCalled();
    expect(terminal.clearSelection).not.toHaveBeenCalled();
  });

  it("reports a missing clipboard as a failure and still clears", async () => {
    vi.stubGlobal("navigator", {});
    const terminal = fakeTerminal(() => "selected");

    expect(await copyTerminalSelection(terminal)).toBe(false);
    expect(terminal.clearSelection).toHaveBeenCalledTimes(1);
  });
});
