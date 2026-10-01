// @vitest-environment happy-dom
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  clickDialogButton,
  clickMenuEntry,
  dialog,
  lifecycleClose,
  plainClick,
  renderWorkspace,
  resizeWindow,
  settleCloseActs,
  tabElement,
} from "./bulkCloseHarness";
import { sessionStop } from "../../lib/tauri";

beforeEach(beforeEachHarness);
afterEach(afterEachHarness);

describe("pane session close confirmation", () => {
  it("opens the real pane kebab and traps focus with Cancel initially focused", async () => {
    await renderWorkspace();
    await plainClick("session-2");
    const kebab = document.querySelector<HTMLButtonElement>(".pane-header-kebab");
    expect(kebab?.getAttribute("aria-expanded")).toBe("false");
    await act(async () => kebab?.click());
    expect(kebab?.getAttribute("aria-expanded")).toBe("true");
    expect(document.querySelector('[role="menu"]')?.textContent).toContain("Close");
    await clickMenuEntry("Close");
    const ask = dialog();
    expect(ask.textContent).toContain("The process stops and every message stays in History.");
    const cancel = ask.querySelector<HTMLButtonElement>(".confirm-dialog-cancel");
    const confirm = ask.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");
    expect(document.activeElement).toBe(cancel);
    await act(async () =>
      cancel?.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Tab",
          shiftKey: true,
          bubbles: true,
          cancelable: true,
        }),
      ),
    );
    expect(document.activeElement).toBe(confirm);
    await act(async () =>
      confirm?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }),
      ),
    );
    expect(document.activeElement).toBe(cancel);
    expect(sessionStop).not.toHaveBeenCalled();
  });

  it("keeps the confirmation through resize", async () => {
    await renderWorkspace();
    await lifecycleClose("session-2");
    const ask = dialog();
    await resizeWindow();
    expect(dialog()).toBe(ask);
    expect(sessionStop).not.toHaveBeenCalled();
    await clickDialogButton("Close");
    expect(sessionStop).toHaveBeenCalledWith("session-2");
  });

  it.each(["success", "refusal"])(
    "keeps later navigation when session_stop settles with %s",
    async (outcome) => {
      let resolve!: () => void;
      let reject!: (error: Error) => void;
      vi.mocked(sessionStop).mockImplementationOnce(
        () =>
          new Promise<void>((yes, no) => {
            resolve = yes;
            reject = no;
          }),
      );
      await renderWorkspace();
      await lifecycleClose("session-2");
      await clickDialogButton("Close");
      expect(tabElement("session-3").getAttribute("aria-selected")).toBe("true");
      await plainClick("agent-one");
      await act(async () => (outcome === "success" ? resolve() : reject(new Error("refused"))));
      await settleCloseActs();
      expect(tabElement("agent-one").getAttribute("aria-selected")).toBe("true");
      if (outcome === "refusal")
        expect(tabElement("session-2").getAttribute("aria-selected")).toBe("false");
    },
  );

  it("restores a refused active tab after the strip selected its successor", async () => {
    let refuse!: (error: Error) => void;
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((_resolve, reject) => {
          refuse = reject;
        }),
    );
    await renderWorkspace();
    await lifecycleClose("session-2");
    await clickDialogButton("Close");
    expect(tabElement("session-3").getAttribute("aria-selected")).toBe("true");
    await act(async () => refuse(new Error("refused")));
    await settleCloseActs();
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabElement("session-2"));
  });
});
