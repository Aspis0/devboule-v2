// @vitest-environment happy-dom

// The paired row's kebab: Revoke and Lost-or-stolen arming, operable by
// pointer and keyboard, with focus returned on close. The menu names the
// device by its display name — never the raw device id.
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
    vi.clearAllMocks();
  });

  function kebabButton(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".dev-kebab");
    if (button === null) throw new Error("kebab button did not render");
    return button;
  }

  function menuItems(): HTMLButtonElement[] {
    return Array.from(container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
  }

  async function renderKebab(props: Partial<Parameters<typeof DeviceKebab>[0]> = {}) {
    await act(async () =>
      root.render(
        <DeviceKebab displayName="Xiaomi 14" onRevoke={() => {}} onLost={() => {}} {...props} />,
      ),
    );
  }

  async function openMenu() {
    await act(async () => kebabButton().click());
  }

  it("names the device by its display name, never the raw id", async () => {
    await renderKebab({ displayName: "Xiaomi 14" });
    expect(kebabButton().getAttribute("aria-label")).toBe("Actions for Xiaomi 14");
    await openMenu();
    const menu = container.querySelector('[role="menu"]');
    if (menu === null) throw new Error("kebab menu did not render");
    expect(menu.getAttribute("aria-label")).toBe("Actions for Xiaomi 14");
    expect(container.textContent).not.toContain("9f6b0f2e");
  });

  it("stays closed until opened, then offers both revoke paths", async () => {
    await renderKebab();
    expect(kebabButton().getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector('[role="menu"]')).toBeNull();
    await openMenu();
    expect(kebabButton().getAttribute("aria-expanded")).toBe("true");
    const names = menuItems().map((item) => item.textContent);
    expect(names).toContain("Revoke");
    expect(names).toContain("Lost or stolen device");
  });

  it("arms the standard revoke through and closes", async () => {
    const onRevoke = vi.fn();
    await renderKebab({ onRevoke });
    await openMenu();
    const revoke = menuItems().find((item) => item.textContent === "Revoke");
    if (revoke === undefined) throw new Error("Revoke item did not render");
    await act(async () => revoke.click());
    expect(onRevoke).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("arms the lost-device path through and closes", async () => {
    const onLost = vi.fn();
    await renderKebab({ onLost });
    await openMenu();
    const lost = menuItems().find((item) => item.textContent === "Lost or stolen device");
    if (lost === undefined) throw new Error("lost-device item did not render");
    await act(async () => lost.click());
    expect(onLost).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("returns focus to the button on Escape", async () => {
    await renderKebab();
    await openMenu();
    const items = menuItems();
    if (items.length === 0) throw new Error("kebab menu did not render");
    items[0]?.focus();
    expect(document.activeElement).not.toBe(kebabButton());
    await act(async () => {
      items[0]?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
      );
    });
    expect(container.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(kebabButton());
  });

  it("moves through the items with the arrow keys", async () => {
    await renderKebab();
    await openMenu();
    const items = menuItems();
    if (items.length < 2) throw new Error("kebab menu did not render two items");
    items[0]?.focus();
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }),
      );
    });
    expect(document.activeElement).toBe(items[1]);
  });
});
