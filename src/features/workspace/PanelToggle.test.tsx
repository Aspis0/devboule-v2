// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PanelStub, PanelToggle } from "./PanelToggle";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("the side-panel toggles", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    vi.stubGlobal("navigator", { platform: "Win32" });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  function button(label: string): HTMLButtonElement {
    const found = container.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
    if (found === null) throw new Error(`no ${label} button`);
    return found;
  }

  it("collapses the sidebar from its header toggle, named and marked expanded", () => {
    const onCollapse = vi.fn();
    act(() => root.render(<PanelToggle side="left" onCollapse={onCollapse} />));

    const toggle = button("Collapse sidebar");
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    act(() => toggle.click());
    expect(onCollapse).toHaveBeenCalledTimes(1);
  });

  it("prints its label and key in a tip that assistive tech reads through the button's name", () => {
    act(() => root.render(<PanelToggle side="left" onCollapse={() => undefined} />));

    const tip = button("Collapse sidebar").querySelector<HTMLElement>(".panel-key-tip");
    expect(tip?.getAttribute("aria-hidden")).toBe("true");
    expect(tip?.querySelector(".panel-key-tip-label")?.textContent).toBe("Collapse sidebar");
    const keys = [...(tip?.querySelectorAll("kbd") ?? [])].map((key) => key.textContent);
    expect(keys).toEqual(["Ctrl", "B"]);
  });

  it("uses E for the right panel, and its own name", () => {
    act(() => root.render(<PanelToggle side="right" onCollapse={() => undefined} />));

    const toggle = button("Collapse panel");
    const keys = [...toggle.querySelectorAll("kbd")].map((key) => key.textContent);
    expect(keys).toEqual(["Ctrl", "E"]);
  });

  it("shows the command key as ⌘ on macOS", () => {
    vi.stubGlobal("navigator", { platform: "MacIntel" });
    act(() => root.render(<PanelToggle side="left" onCollapse={() => undefined} />));

    const keys = [...button("Collapse sidebar").querySelectorAll("kbd")].map(
      (key) => key.textContent,
    );
    expect(keys).toEqual(["⌘", "B"]);
  });

  it("stands in for a collapsed side as a stub that shows it again", () => {
    const onShow = vi.fn();
    act(() => root.render(<PanelStub side="left" onShow={onShow} />));

    const stub = button("Show sidebar");
    expect(stub.getAttribute("aria-expanded")).toBe("false");
    expect(stub.querySelector(".workspace-vertical-label")?.textContent).toBe("workspaces");
    act(() => stub.click());
    expect(onShow).toHaveBeenCalledTimes(1);
  });

  it("takes focus when the collapse left none, so the keyboard keeps its place", () => {
    (document.activeElement as HTMLElement | null)?.blur();
    act(() => root.render(<PanelStub side="right" onShow={() => undefined} />));

    expect(document.activeElement).toBe(button("Show panel"));
  });

  it("leaves focus where it is when a chord collapses the panel from elsewhere", () => {
    const composer = document.createElement("textarea");
    document.body.appendChild(composer);
    composer.focus();
    act(() => root.render(<PanelStub side="right" onShow={() => undefined} />));

    expect(document.activeElement).toBe(composer);
    composer.remove();
  });
});
