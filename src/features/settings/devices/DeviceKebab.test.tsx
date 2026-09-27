// @vitest-environment happy-dom

// The paired row's kebab: Revoke and Lost-or-stolen arming, operable by
// pointer and keyboard. The menu names the device by its display name —
// never the raw device id — renders through the house portal (so the
// settings scroll container cannot clip it), and never autofocuses a
// destructive item: focus stays on the trigger until an arrow key enters
// the menu, and returns to the trigger on close.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DeviceKebab } from "./DeviceKebab";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("DeviceKebab", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  function kebabButton(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".dev-kebab");
    if (button === null) throw new Error("kebab button did not render");
    return button;
  }

  function openMenu(): HTMLElement {
    const menu = document.body.querySelector<HTMLElement>('[role="menu"]');
    if (menu === null) throw new Error("kebab menu did not render");
    return menu;
  }

  function menuItems(): HTMLButtonElement[] {
    return Array.from(openMenu().querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
  }

  async function renderKebab(props: Partial<Parameters<typeof DeviceKebab>[0]> = {}) {
    await act(async () =>
      root.render(
        <DeviceKebab displayName="Xiaomi 14" onRevoke={() => {}} onLost={() => {}} {...props} />,
      ),
    );
  }

  async function renderTwoKebabs() {
    await act(async () =>
      root.render(
        <>
          <DeviceKebab displayName="First" onRevoke={() => {}} onLost={() => {}} />
          <DeviceKebab displayName="Second" onRevoke={() => {}} onLost={() => {}} />
        </>,
      ),
    );
  }

  async function openFirstMenu() {
    await act(async () => kebabButton().click());
  }

  function keyDown(target: Element, key: string) {
    target.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
  }

  it("names the device by its display name, never the raw id", async () => {
    await renderKebab({ displayName: "Xiaomi 14" });
    expect(kebabButton().getAttribute("aria-label")).toBe("Actions for Xiaomi 14");
    expect(kebabButton().getAttribute("aria-haspopup")).toBe("menu");
    await openFirstMenu();
    expect(openMenu().getAttribute("aria-label")).toBe("Actions for Xiaomi 14");
    expect(document.body.textContent).not.toContain("9f6b0f2e");
  });

  it("stays closed until opened, then offers both revoke paths", async () => {
    await renderKebab();
    expect(kebabButton().getAttribute("aria-expanded")).toBe("false");
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    await openFirstMenu();
    expect(kebabButton().getAttribute("aria-expanded")).toBe("true");
    const names = menuItems().map((item) => item.textContent);
    expect(names).toContain("Revoke");
    expect(names).toContain("Lost or stolen device");
  });

  it("renders the menu in a portal the scroll container cannot clip", async () => {
    await renderKebab();
    await openFirstMenu();
    const menu = openMenu();
    // The menu is on document.body, not inside the row: no ancestor
    // overflow (`.settings-main`'s scroll port) clips it, and the house
    // popover placement flips it above the anchor when the space below
    // is smaller.
    expect(menu.parentElement).toBe(document.body);
    expect(container.contains(menu)).toBe(false);
  });

  it("leaves focus on the trigger when the menu opens", async () => {
    await renderKebab();
    kebabButton().focus();
    await openFirstMenu();
    // Neither destructive item may hold focus on open: a second Enter
    // must not arm a revoke from an icon button alone.
    expect(document.activeElement).toBe(kebabButton());
  });

  it("enters the menu with ArrowDown from the trigger", async () => {
    await renderKebab();
    kebabButton().focus();
    await openFirstMenu();
    await act(async () => keyDown(kebabButton(), "ArrowDown"));
    expect(document.activeElement).toBe(menuItems()[0]);
  });

  it("travels with arrows and jumps with Home and End", async () => {
    await renderKebab();
    kebabButton().focus();
    await openFirstMenu();
    await act(async () => keyDown(kebabButton(), "ArrowDown"));
    expect(document.activeElement).toBe(menuItems()[0]);
    await act(async () => keyDown(document.activeElement!, "ArrowDown"));
    expect(document.activeElement).toBe(menuItems()[1]);
    await act(async () => keyDown(document.activeElement!, "ArrowDown"));
    expect(document.activeElement).toBe(menuItems()[0]);
    await act(async () => keyDown(document.activeElement!, "End"));
    expect(document.activeElement).toBe(menuItems()[1]);
    await act(async () => keyDown(document.activeElement!, "Home"));
    expect(document.activeElement).toBe(menuItems()[0]);
    await act(async () => keyDown(document.activeElement!, "ArrowUp"));
    expect(document.activeElement).toBe(menuItems()[1]);
  });

  it("arms the standard revoke through, closes, and returns focus", async () => {
    const onRevoke = vi.fn();
    await renderKebab({ onRevoke });
    await openFirstMenu();
    const revoke = menuItems().find((item) => item.textContent === "Revoke");
    if (revoke === undefined) throw new Error("Revoke item did not render");
    await act(async () => revoke.click());
    expect(onRevoke).toHaveBeenCalledTimes(1);
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(kebabButton());
  });

  it("arms the lost-device path through and closes", async () => {
    const onLost = vi.fn();
    await renderKebab({ onLost });
    await openFirstMenu();
    const lost = menuItems().find((item) => item.textContent === "Lost or stolen device");
    if (lost === undefined) throw new Error("lost-device item did not render");
    await act(async () => lost.click());
    expect(onLost).toHaveBeenCalledTimes(1);
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
  });

  it("returns focus to the button on Escape", async () => {
    await renderKebab();
    await openFirstMenu();
    const items = menuItems();
    if (items.length === 0) throw new Error("kebab menu did not render");
    items[0]?.focus();
    expect(document.activeElement).not.toBe(kebabButton());
    await act(async () => keyDown(document.activeElement!, "Escape"));
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(kebabButton());
  });

  it("closes on Escape from the trigger", async () => {
    await renderKebab();
    kebabButton().focus();
    await openFirstMenu();
    await act(async () => keyDown(kebabButton(), "Escape"));
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(kebabButton());
  });

  it("closes when focus moves outside the menu and the trigger", async () => {
    // Tab away from the open menu: focus lands on content the menu floats
    // over, so the menu must go with it — no orphan `role="menu"` with
    // `aria-expanded="true"` and focus somewhere else in the page.
    await renderKebab();
    await openFirstMenu();
    const outside = document.createElement("button");
    outside.textContent = "elsewhere";
    document.body.appendChild(outside);
    await act(async () => {
      outside.focus();
      outside.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
    });
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
    expect(kebabButton().getAttribute("aria-expanded")).toBe("false");
  });

  it("stays open when focus moves between the trigger and the menu", async () => {
    await renderKebab();
    kebabButton().focus();
    await openFirstMenu();
    await act(async () => {
      kebabButton().dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
    });
    expect(document.body.querySelector('[role="menu"]')).not.toBeNull();
    const items = menuItems();
    if (items.length === 0) throw new Error("kebab menu did not render");
    await act(async () => {
      items[0]?.focus();
      items[0]?.dispatchEvent(new FocusEvent("focusin", { bubbles: true }));
    });
    expect(document.body.querySelector('[role="menu"]')).not.toBeNull();
  });

  it("closes on an outside press", async () => {
    await renderKebab();
    await openFirstMenu();
    expect(document.body.querySelector('[role="menu"]')).not.toBeNull();
    await act(async () => {
      document.body.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    });
    expect(document.body.querySelector('[role="menu"]')).toBeNull();
  });

  it("keeps one menu open at a time across rows", async () => {
    await renderTwoKebabs();
    const buttons = Array.from(container.querySelectorAll<HTMLButtonElement>(".dev-kebab"));
    if (buttons.length !== 2) throw new Error("two kebabs did not render");
    await act(async () => buttons[0]?.click());
    expect(document.body.querySelectorAll('[role="menu"]')).toHaveLength(1);
    await act(async () => buttons[1]?.click());
    const menus = Array.from(document.body.querySelectorAll('[role="menu"]'));
    expect(menus).toHaveLength(1);
    expect(menus[0]?.getAttribute("aria-label")).toBe("Actions for Second");
  });
});
