// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePanelChords } from "./usePanelChords";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function Harness(props: {
  leftCollapsed: boolean;
  rightCollapsed: boolean;
  setLeftCollapsed: (collapsed: boolean) => void;
  setRightCollapsed: (collapsed: boolean) => void;
}) {
  usePanelChords(props);
  return null;
}

function press(key: string, target: EventTarget = document): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    ctrlKey: true,
    bubbles: true,
    cancelable: true,
  });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
}

describe("usePanelChords", () => {
  let container: HTMLDivElement;
  let root: Root;
  const setLeft = vi.fn();
  const setRight = vi.fn();

  beforeEach(() => {
    vi.stubGlobal("navigator", { platform: "Win32" });
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    setLeft.mockReset();
    setRight.mockReset();
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  function mount(leftCollapsed: boolean, rightCollapsed: boolean) {
    act(() => {
      root.render(
        <Harness
          leftCollapsed={leftCollapsed}
          rightCollapsed={rightCollapsed}
          setLeftCollapsed={setLeft}
          setRightCollapsed={setRight}
        />,
      );
    });
  }

  it("toggles the sidebar on Ctrl+B and the panel on Ctrl+E", () => {
    mount(false, false);
    const sidebar = press("b");
    expect(setLeft).toHaveBeenCalledExactlyOnceWith(true);
    expect(sidebar.defaultPrevented).toBe(true);

    press("e");
    expect(setRight).toHaveBeenCalledExactlyOnceWith(true);
    expect(setLeft).toHaveBeenCalledTimes(1);
  });

  it("collapses both when either panel is open", () => {
    mount(false, true);
    press(".");
    expect(setLeft).toHaveBeenCalledExactlyOnceWith(true);
    expect(setRight).toHaveBeenCalledExactlyOnceWith(true);
  });

  it("opens both when both are collapsed", () => {
    mount(true, true);
    press(".");
    expect(setLeft).toHaveBeenCalledExactlyOnceWith(false);
    expect(setRight).toHaveBeenCalledExactlyOnceWith(false);
  });

  it("leaves Ctrl+B alone inside a terminal, where the shell owns it", () => {
    mount(false, false);
    const host = document.createElement("div");
    host.innerHTML = '<div class="workspace-terminal-shell"><textarea></textarea></div>';
    document.body.appendChild(host);
    const textarea = host.querySelector("textarea") as HTMLElement;

    const event = press("b", textarea);

    expect(setLeft).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
    host.remove();
  });
});
