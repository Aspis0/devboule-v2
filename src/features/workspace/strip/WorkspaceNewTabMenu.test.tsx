// @vitest-environment happy-dom

// The + menu component in isolation: which entries disable (a create in
// flight disables all; without a selected workspace all wait, and the menu
// says why), and the menu-scoped keyboard — Home/End and wrapping arrows among
// the ENABLED entries, and Escape closing with focus back on the trigger. Tab
// closing is pinned through the real wiring in WorkspaceNewTab.test.tsx.

import { act, useRef } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NO_WORKSPACE_REASON_ID, WorkspaceNewTabMenu } from "./WorkspaceNewTabMenu";

interface HarnessProps {
  creating: boolean;
  workspaceSelected: boolean;
  onClose: () => void;
  onAgent: () => void;
  onTerminal: () => void;
  onBrowser: () => void;
}

function Harness({
  creating,
  workspaceSelected,
  onClose,
  onAgent,
  onTerminal,
  onBrowser,
}: HarnessProps) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button type="button" ref={triggerRef} data-testid="trigger">
        +
      </button>
      <WorkspaceNewTabMenu
        open
        triggerRef={triggerRef}
        creating={creating}
        workspaceSelected={workspaceSelected}
        onAgent={onAgent}
        onTerminal={onTerminal}
        onBrowser={onBrowser}
        onClose={onClose}
      />
    </>
  );
}

function renderMenu(options: { creating?: boolean; workspaceSelected?: boolean } = {}) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const menu = {
    onClose: vi.fn(),
    onAgent: vi.fn(),
    onTerminal: vi.fn(),
    onBrowser: vi.fn(),
  };
  act(() => {
    root.render(
      <Harness
        creating={options.creating ?? false}
        workspaceSelected={options.workspaceSelected ?? true}
        onClose={menu.onClose}
        onAgent={menu.onAgent}
        onTerminal={menu.onTerminal}
        onBrowser={menu.onBrowser}
      />,
    );
  });
  const entry = (label: string): HTMLButtonElement => {
    const item = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (button) => button.textContent === label,
    );
    if (item === undefined) throw new Error(`menu item did not render: ${label}`);
    return item;
  };
  const trigger = (): HTMLButtonElement => {
    const element = container.querySelector<HTMLButtonElement>("[data-testid=trigger]");
    if (element === null) throw new Error("trigger did not render");
    return element;
  };
  const reason = (): HTMLElement | null => document.getElementById(NO_WORKSPACE_REASON_ID);
  return {
    ...menu,
    entry,
    reason,
    trigger,
    unmount: () => {
      act(() => {
        root.unmount();
      });
      container.remove();
    },
  };
}

describe("the + menu entries and keys", () => {
  let cleanup: (() => void) | null = null;
  afterEach(() => {
    if (cleanup !== null) cleanup();
    cleanup = null;
  });

  function render(options?: { creating?: boolean; workspaceSelected?: boolean }) {
    const menu = renderMenu(options);
    cleanup = menu.unmount;
    return menu;
  }

  it("marks every entry aria-disabled while a create is in flight", () => {
    const menu = render({ creating: true });
    for (const label of ["Agent", "Terminal", "Browser"]) {
      expect(menu.entry(label).getAttribute("aria-disabled")).toBe("true");
      expect(menu.entry(label).disabled).toBe(false);
    }
  });

  it("marks every entry aria-disabled when no workspace is selected, and says why", () => {
    const menu = render({ workspaceSelected: false });
    for (const label of ["Agent", "Terminal", "Browser"]) {
      expect(menu.entry(label).getAttribute("aria-disabled")).toBe("true");
      expect(menu.entry(label).disabled).toBe(false);
    }
    const reason = menu.reason();
    expect(reason?.textContent).toBe("No workspace is selected.");
    expect(menu.entry("Agent").getAttribute("aria-describedby")).toBe(reason?.id);
    expect(menu.entry("Browser").getAttribute("aria-describedby")).toBe(reason?.id);
  });

  it("keeps a waiting menu reachable by keyboard: focus, arrows, End and Escape", () => {
    // A disabled attribute would drop these entries from focus and leave the
    // reason unreachable; aria-disabled keeps them in the keyboard path.
    const menu = render({ workspaceSelected: false });
    const agent = menu.entry("Agent");
    const terminal = menu.entry("Terminal");
    const browser = menu.entry("Browser");
    expect(document.activeElement).toBe(agent);
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(terminal);
    act(() => {
      terminal.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    });
    expect(document.activeElement).toBe(browser);
    act(() => {
      browser.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(menu.onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(menu.trigger());
  });

  it("does nothing when a waiting entry is activated", () => {
    const menu = render({ workspaceSelected: false });
    act(() => menu.entry("Agent").click());
    act(() => menu.entry("Terminal").click());
    act(() => menu.entry("Browser").click());
    expect(menu.onAgent).not.toHaveBeenCalled();
    expect(menu.onTerminal).not.toHaveBeenCalled();
    expect(menu.onBrowser).not.toHaveBeenCalled();
    expect(menu.onClose).not.toHaveBeenCalled();
  });

  it("shows no reason while a workspace is selected", () => {
    const menu = render();
    expect(menu.reason()).toBeNull();
    expect(menu.entry("Agent").getAttribute("aria-describedby")).toBeNull();
  });

  it("Home and End move to the first and last entries from real focus positions", () => {
    const menu = render();
    const agent = menu.entry("Agent");
    const browser = menu.entry("Browser");
    // Opening the menu focuses the first entry; the keys are dispatched on
    // the element that has focus, as a real keypress would be.
    expect(document.activeElement).toBe(agent);
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    });
    expect(document.activeElement).toBe(browser);
    act(() => {
      browser.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true }));
    });
    expect(document.activeElement).toBe(agent);
  });

  it("ArrowDown moves between the entries and wraps", () => {
    const menu = render();
    const agent = menu.entry("Agent");
    const terminal = menu.entry("Terminal");
    const browser = menu.entry("Browser");
    act(() => agent.focus());
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(terminal);
    act(() => {
      terminal.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(browser);
    act(() => {
      browser.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(agent);
  });

  it("closes on an outside pointerdown, not on mousedown", () => {
    const menu = render();
    const agent = menu.entry("Agent");
    // A press inside the menu never dismisses it.
    act(() => {
      agent.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    });
    expect(menu.onClose).not.toHaveBeenCalled();
    // The legacy mouse event is not the outside press.
    act(() => {
      document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(menu.onClose).not.toHaveBeenCalled();
    // Any pointer press outside the menu and its trigger does.
    act(() => {
      document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    });
    expect(menu.onClose).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape and returns focus to the trigger", () => {
    const menu = render();
    const agent = menu.entry("Agent");
    act(() => agent.focus());
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(menu.onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(menu.trigger());
  });
});
