// @vitest-environment happy-dom

// The row kebab: Update (only when offered), Refresh, copy path — operable
// by pointer and keyboard, with focus returned on close.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderKebab } from "./ProviderKebab";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("ProviderKebab", () => {
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
    vi.unstubAllGlobals();
  });

  function kebabButton(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!button) throw new Error("kebab button did not render");
    return button;
  }

  function menuItems(): HTMLButtonElement[] {
    return Array.from(container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
  }

  async function renderKebab(props: Partial<Parameters<typeof ProviderKebab>[0]> = {}) {
    await act(async () =>
      root.render(
        <ProviderKebab providerId="grok" path="C:\npm\grok.cmd" onRefresh={() => {}} {...props} />,
      ),
    );
  }

  async function openMenu() {
    await act(async () => kebabButton().click());
  }

  it("stays closed until opened, then offers Refresh and copy path", async () => {
    await renderKebab();
    expect(kebabButton().getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector('[role="menu"]')).toBeNull();
    await openMenu();
    expect(kebabButton().getAttribute("aria-expanded")).toBe("true");
    const names = menuItems().map((item) => item.textContent);
    expect(names).toContain("Refresh");
    expect(names).toContain("Copy path");
    expect(names).not.toContain("Update");
  });

  it("offers Update only when the row can update, and calls through on choice", async () => {
    const onUpdate = vi.fn();
    await renderKebab({ onUpdate });
    await openMenu();
    const update = menuItems().find((item) => item.textContent === "Update");
    if (!update) throw new Error("Update item did not render");
    await act(async () => update.click());
    expect(onUpdate).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("runs Refresh through and closes", async () => {
    const onRefresh = vi.fn();
    await renderKebab({ onRefresh });
    await openMenu();
    const refresh = menuItems().find((item) => item.textContent === "Refresh");
    if (!refresh) throw new Error("Refresh item did not render");
    await act(async () => refresh.click());
    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("copies the executable path and says so", async () => {
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    await renderKebab();
    await openMenu();
    const copy = menuItems().find((item) => item.textContent === "Copy path");
    if (!copy) throw new Error("Copy-path item did not render");
    await act(async () => copy.click());
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith("C:\\npm\\grok.cmd");
    expect(container.textContent).toContain("Copied");
  });

  it("says the copy failed instead of going silent", async () => {
    const writeText = vi.fn(async () => {
      throw new Error("denied");
    });
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    await renderKebab();
    await openMenu();
    const copy = menuItems().find((item) => item.textContent === "Copy path");
    if (!copy) throw new Error("Copy-path item did not render");
    await act(async () => copy.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("Copy failed");
  });

  it("closes on Escape and returns focus to the kebab button", async () => {
    await renderKebab({ onUpdate: () => {} });
    await openMenu();
    const first = menuItems()[0];
    if (!first) throw new Error("menu items did not render");
    expect(document.activeElement).toBe(first);
    await act(async () => {
      first.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(kebabButton());
  });

  it("moves between items with ArrowDown and ArrowUp", async () => {
    await renderKebab({ onUpdate: () => {} });
    await openMenu();
    const items = menuItems();
    expect(items.length).toBeGreaterThan(1);
    expect(document.activeElement).toBe(items[0]);
    await act(async () => {
      items[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    expect(document.activeElement).toBe(items[1]);
    await act(async () => {
      items[1].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
    });
    expect(document.activeElement).toBe(items[0]);
  });

  it("closes on Tab so focus can move on without an orphan menu", async () => {
    await renderKebab({ onUpdate: () => {} });
    await openMenu();
    const first = menuItems()[0];
    if (!first) throw new Error("menu items did not render");
    await act(async () => {
      first.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    });
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("closes on outside pointer down", async () => {
    await renderKebab();
    await openMenu();
    expect(container.querySelector('[role="menu"]')).not.toBeNull();
    await act(async () => {
      document.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    });
    expect(container.querySelector('[role="menu"]')).toBeNull();
  });

  it("keeps only one menu open across rows", async () => {
    await act(async () =>
      root.render(
        <>
          <ProviderKebab providerId="aaa" path="x" onRefresh={() => {}} />
          <ProviderKebab providerId="bbb" path="x" onRefresh={() => {}} />
        </>,
      ),
    );
    const buttons = container.querySelectorAll<HTMLButtonElement>(".prov-kebab");
    expect(buttons).toHaveLength(2);
    await act(async () => buttons[0]?.click());
    expect(container.querySelectorAll('[role="menu"]')).toHaveLength(1);
    await act(async () => buttons[1]?.click());
    const menus = container.querySelectorAll('[role="menu"]');
    expect(menus).toHaveLength(1);
    expect(menus[0]?.getAttribute("aria-label")).toContain("bbb");
  });
});
