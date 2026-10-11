// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { surfaceSettingsGet, surfaceSettingsSet } from "../../lib/tauri";
import { CloseBehaviorSetting } from "./CloseBehaviorSetting";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    surfaceSettingsGet: vi.fn(),
    surfaceSettingsSet: vi.fn(),
  };
});

const getMock = vi.mocked(surfaceSettingsGet);
const setMock = vi.mocked(surfaceSettingsSet);

function optionFor(container: ParentNode, value: string): HTMLElement {
  const found = container.querySelector<HTMLElement>(
    `[role="radio"][data-choice-value="${value}"]`,
  );
  if (!found) throw new Error(`the ${value} choice did not render`);
  return found;
}

function isChosen(container: ParentNode, value: string): boolean {
  return optionFor(container, value).getAttribute("aria-checked") === "true";
}

async function choose(container: ParentNode, value: string): Promise<void> {
  await act(async () => {
    optionFor(container, value).click();
  });
}

describe("CloseBehaviorSetting", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.resetAllMocks();
  });

  it("loads the stored choice instead of assuming ask", async () => {
    getMock.mockResolvedValue({ status: "value", value: { choice: "tray" } });
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    expect(getMock).toHaveBeenCalledWith("close-behavior");
    expect(isChosen(container, "tray")).toBe(true);
  });

  it("offers the three choices as one visible set, not a closed select", async () => {
    getMock.mockResolvedValue({ status: "absent" });
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    // A select hides the alternatives behind a second interaction; the
    // segmented control keeps all three visible.
    expect(container.querySelector("select")).toBeNull();
    const group = container.querySelector('[role="radiogroup"]');
    expect(group?.textContent).toContain("Ask every time");
    expect(group?.textContent).toContain("Keep running in the tray");
    expect(group?.textContent).toContain("Quit Devboule");
    // One row, one line: the title and the control, no explanatory sentence.
    expect(container.querySelector("[data-settings-row-description]")).toBeNull();
  });

  it("saves the chosen choice through the surface settings", async () => {
    getMock.mockResolvedValue({ status: "absent" });
    setMock.mockResolvedValue(undefined);
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    await choose(container, "quit");
    expect(setMock).toHaveBeenCalledWith("close-behavior", { choice: "quit" });
  });

  it("reports a rejected read and keeps the row usable", async () => {
    getMock.mockRejectedValue(new Error("the bridge is gone"));
    setMock.mockResolvedValue(undefined);
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    expect(container.querySelector("[role=alert]")?.textContent).toContain("the bridge is gone");
    await choose(container, "tray");
    expect(setMock).toHaveBeenCalledWith("close-behavior", { choice: "tray" });
  });

  it("shows the mapped sentence for a read the daemon refuses, never its raw words", async () => {
    getMock.mockRejectedValue({
      code: "io",
      message: "Could not read the settings file: Access is denied. (os error 5)",
    });
    setMock.mockResolvedValue(undefined);
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    const alert = container.querySelector("[role=alert]")?.textContent ?? "";
    expect(alert).toBe("A system or file operation failed on this machine.");
    expect(alert).not.toContain("os error");
    expect(container.querySelector('[role="radio"]')).not.toBeNull();
  });

  it("restores the stored choice when the save fails", async () => {
    getMock.mockResolvedValue({ status: "value", value: { choice: "ask" } });
    setMock.mockRejectedValue({ code: "io", message: "the disk said no" });
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    await choose(container, "quit");
    await act(async () => undefined); // let the failure settle
    expect(isChosen(container, "ask")).toBe(true);
    expect(container.querySelector("[role=alert]")?.textContent).toBe(
      "A system or file operation failed on this machine.",
    );
  });

  it("shows the mapped sentence for a save the daemon refuses, never its raw words", async () => {
    getMock.mockResolvedValue({ status: "absent" });
    setMock.mockRejectedValue({
      code: "io",
      message: "Could not write the settings file: Access is denied. (os error 5)",
    });
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    await choose(container, "quit");
    await act(async () => undefined);
    const alert = container.querySelector("[role=alert]")?.textContent ?? "";
    expect(alert).toBe("A system or file operation failed on this machine.");
    expect(alert).not.toContain("os error");
  });
});
