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

function radioFor(container: ParentNode, value: string): HTMLInputElement {
  const found = container.querySelector<HTMLInputElement>(
    `input[name="close-behavior"][value="${value}"]`,
  );
  if (!found) throw new Error(`the ${value} choice did not render`);
  return found;
}

async function choose(container: ParentNode, value: string): Promise<void> {
  await act(async () => {
    radioFor(container, value).click();
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
    expect(radioFor(container, "tray").checked).toBe(true);
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

  it("restores the stored choice when the save fails", async () => {
    getMock.mockResolvedValue({ status: "value", value: { choice: "ask" } });
    setMock.mockRejectedValue({ message: "the disk said no" });
    root = createRoot(container);
    await act(async () => root.render(<CloseBehaviorSetting />));
    await choose(container, "quit");
    await act(async () => undefined); // let the failure settle
    expect(radioFor(container, "ask").checked).toBe(true);
    expect(container.querySelector("[role=alert]")?.textContent).toContain("the disk said no");
  });
});
