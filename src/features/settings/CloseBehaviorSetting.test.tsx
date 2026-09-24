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
    const select = container.querySelector<HTMLSelectElement>("select");
    expect(select?.value).toBe("tray");
  });

  it("saves the chosen choice through the surface settings", async () => {
    getMock.mockResolvedValue({ status: "absent" });
    setMock.mockResolvedValue(undefined);
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    const select = container.querySelector<HTMLSelectElement>("select");
    if (!select) throw new Error("the choice select did not render");
    await act(async () => {
      select.value = "quit";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(setMock).toHaveBeenCalledWith("close-behavior", { choice: "quit" });
  });

  it("reports a rejected read and keeps the row usable", async () => {
    getMock.mockRejectedValue(new Error("the bridge is gone"));
    setMock.mockResolvedValue(undefined);
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    expect(container.querySelector("[role=alert]")?.textContent).toContain("the bridge is gone");
    const select = container.querySelector<HTMLSelectElement>("select");
    if (!select) throw new Error("the choice select did not render");
    await act(async () => {
      select.value = "tray";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(setMock).toHaveBeenCalledWith("close-behavior", { choice: "tray" });
  });

  it("restores the stored choice when the save fails", async () => {
    getMock.mockResolvedValue({ status: "value", value: { choice: "ask" } });
    setMock.mockRejectedValue({ message: "the disk said no" });
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    const select = container.querySelector<HTMLSelectElement>("select");
    if (!select) throw new Error("the choice select did not render");
    await act(async () => {
      select.value = "quit";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => undefined); // let the failure settle
    expect(select.value).toBe("ask");
    expect(container.querySelector("[role=alert]")?.textContent).toContain("the disk said no");
  });
});
