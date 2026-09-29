// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import { createTerminalView } from "./createTerminalView";

async function tick(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

/**
 * Ctrl+Shift+V is the browser's own paste: the keydown must stay
 * unprevented so the default fires exactly one paste event on xterm's
 * textarea — the same road plain Ctrl+V takes, permission-free.
 */
describe("the Ctrl+Shift+V paste road", () => {
  it("passes the chord unprevented and one native paste lands exactly once", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const onData = vi.fn();
    const view = createTerminalView(host, { onData, onCtrlC: () => undefined });
    const textarea = host.querySelector(".xterm-helper-textarea");
    expect(textarea).not.toBeNull();

    const keydown = new KeyboardEvent("keydown", {
      key: "V",
      ctrlKey: true,
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    textarea!.dispatchEvent(keydown);
    await tick();
    expect(keydown.defaultPrevented).toBe(false);
    expect(onData).not.toHaveBeenCalled();

    const paste = new Event("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(paste, "clipboardData", {
      value: { getData: () => "pasted text" },
    });
    textarea!.dispatchEvent(paste);
    await tick();
    expect(onData).toHaveBeenCalledTimes(1);
    expect(onData).toHaveBeenCalledWith("pasted text");

    view.dispose();
  });
});
