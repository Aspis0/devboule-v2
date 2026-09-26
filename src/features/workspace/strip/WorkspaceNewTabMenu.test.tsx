// @vitest-environment happy-dom

// The + menu component in isolation: which entries disable (a create in
// flight disables both; without a selected workspace Terminal alone waits)
// and the menu-scoped keyboard — Home/End and wrapping arrows among the
// ENABLED entries, and Escape closing with focus back on the trigger. Tab
// closing is pinned through the real wiring in WorkspaceNewTab.test.tsx.

import { act, useRef } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WorkspaceNewTabMenu } from "./WorkspaceNewTabMenu";

interface HarnessProps {
  creating: boolean;
  workspaceSelected: boolean;
  onClose: () => void;
  onAgent: () => void;
  onTerminal: () => void;
}

function Harness({ creating, workspaceSelected, onClose, onAgent, onTerminal }: HarnessProps) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button type="button" ref={triggerRef} data-testid="trigger">
        +
      </button>
      <WorkspaceNewTabMenu
        triggerRef={triggerRef}
        creating={creating}
        workspaceSelected={workspaceSelected}
        onAgent={onAgent}
        onTerminal={onTerminal}
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
  };
  act(() => {
    root.render(
      <Harness
        creating={options.creating ?? false}
        workspaceSelected={options.workspaceSelected ?? true}
        onClose={menu.onClose}
        onAgent={menu.onAgent}
        onTerminal={menu.onTerminal}
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
  return {
    ...menu,
    entry,
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

  it("disables both entries while a create is in flight", () => {
    const menu = render({ creating: true });
    expect(menu.entry("Agent").disabled).toBe(true);
    expect(menu.entry("Terminal").disabled).toBe(true);
  });

  it("disables Terminal alone when no workspace is selected", () => {
    const menu = render({ workspaceSelected: false });
    expect(menu.entry("Agent").disabled).toBe(false);
    expect(menu.entry("Terminal").disabled).toBe(true);
  });

  it("Home and End move to the first and last entries from real focus positions", () => {
    const menu = render();
    const agent = menu.entry("Agent");
    const terminal = menu.entry("Terminal");
    // Opening the menu focuses the first entry; the keys are dispatched on
    // the element that has focus, as a real keypress would be.
    expect(document.activeElement).toBe(agent);
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    });
    expect(document.activeElement).toBe(terminal);
    act(() => {
      terminal.dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true }));
    });
    expect(document.activeElement).toBe(agent);
  });

  it("End and ArrowDown skip a disabled entry: Agent keeps focus", () => {
    // The only enabled entry keeps focus: the arrows never move into a
    // disabled entry, so the menu cannot get stuck on one.
    const menu = render({ workspaceSelected: false });
    const agent = menu.entry("Agent");
    act(() => agent.focus());
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(agent);
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
    });
    expect(document.activeElement).toBe(agent);
  });

  it("ArrowDown moves between the entries and wraps", () => {
    const menu = render();
    const agent = menu.entry("Agent");
    const terminal = menu.entry("Terminal");
    act(() => agent.focus());
    act(() => {
      agent.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(terminal);
    act(() => {
      terminal.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
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
