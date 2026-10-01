// @vitest-environment happy-dom

// The pane header's kebab: entries limited to actions with a working path
// (copy the path when known; the workspace-wired close group), keyboard that
// follows the strip's tab menu, and focus that returns to the kebab.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { buildTabCloseEntries } from "../strip/tabCloseMenu";
import { PaneHeader } from "./PaneHeader";
import { headerMenu, middleTruncate, type PaneHeaderMenu } from "./paneHeaderMenu";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

afterEach(() => {
  // The open menu renders through a body portal: clear it with the hosts.
  document.body.replaceChildren();
});

const DISPLAY = {
  word: "Running",
  detail: null,
  tone: "green",
  pulse: true,
  tooltip: "Running",
} as const;

function renderHeader() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  return { host, root };
}

async function openMenu(menu: PaneHeaderMenu) {
  const { host, root } = renderHeader();
  await act(async () => {
    root.render(<PaneHeader kind="agent" title="Claude" display={DISPLAY} menu={menu} />);
  });
  const kebab = host.querySelector<HTMLButtonElement>(".pane-header-kebab");
  if (kebab === null) throw new Error("kebab did not render");
  await act(async () => {
    kebab.click();
  });
  return { host, root, kebab };
}

function seam() {
  return { closeEntries: buildTabCloseEntries(1, 3), onCloseEntry: vi.fn() };
}

describe("pane header menu entries", () => {
  it("holds Copy path plus the seam's close group, and nothing else", async () => {
    const menu = headerMenu("C:\\Users\\gualt\\Desktop", seam());
    if (menu === null) throw new Error("menu was null");
    const { host, root } = await openMenu(menu);
    const rows = [...document.querySelectorAll("[role='menuitem']")].map((row) => row.textContent);
    expect(rows).toEqual(["Copy path", "Close to the right", "Close other tabs", "Close"]);
    expect(host.textContent).not.toContain("Rename");
    expect(host.textContent).not.toContain("Delete");
    expect(host.textContent).not.toContain("Close to the left");
    await act(async () => root.unmount());
  });

  it("reuses the tab menu's labels verbatim", () => {
    const wired = seam();
    const menu = headerMenu("C:\\x", wired);
    if (menu === null) throw new Error("menu was null");
    const tabLabels = new Map(buildTabCloseEntries(1, 3).map((entry) => [entry.key, entry.label]));
    for (const entry of menu.closeEntries) {
      expect(entry.label).toBe(tabLabels.get(entry.key));
    }
  });

  it("renders no kebab when nothing can act", () => {
    expect(headerMenu(undefined, undefined)).toBeNull();
    expect(headerMenu("", undefined)).toBeNull();
  });

  it("renders the kebab without a path row when the row carries no cwd", async () => {
    const menu = headerMenu(undefined, seam());
    if (menu === null) throw new Error("menu was null without a cwd");
    const { root } = await openMenu(menu);
    expect(document.querySelector(".pane-header-path")).toBeNull();
    const rows = [...document.querySelectorAll("[role='menuitem']")].map((row) => row.textContent);
    expect(rows).toEqual(["Close to the right", "Close other tabs", "Close"]);
    await act(async () => root.unmount());
  });

  it("treats an empty cwd like no cwd", () => {
    expect(headerMenu("", seam())?.copyPath).toBeNull();
  });

  it("preserves a bare verbatim prefix instead of producing an empty copy", () => {
    expect(headerMenu("\\\\?\\", undefined)?.copyPath).toBe("\\\\?\\");
  });

  it("dispatches every close entry with its tab-menu key", async () => {
    const wired = seam();
    const menu = headerMenu("C:\\x", wired);
    if (menu === null) throw new Error("menu was null");
    const { host, root } = await openMenu(menu);
    const keys = ["right", "others", "close"] as const;
    for (const [index, key] of keys.entries()) {
      // Copy path is rows[0]; the close group follows in seam order.
      // Re-queried every pass: firing closes the menu, so last pass's
      // nodes are detached and their clicks would go nowhere.
      const rows = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
      const row = rows[index + 1];
      if (row === undefined) throw new Error(`close entry ${key} did not render`);
      await act(async () => {
        row.click();
      });
      expect(wired.onCloseEntry).toHaveBeenNthCalledWith(index + 1, key);
      if (index < keys.length - 1) {
        await act(async () => {
          host.querySelector<HTMLButtonElement>(".pane-header-kebab")!.click();
        });
      }
    }
    expect(wired.onCloseEntry).toHaveBeenCalledTimes(3);
    await act(async () => root.unmount());
  });
});

describe("pane header menu copy", () => {
  it("copies the full path and says so on the row", async () => {
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const menu = headerMenu("C:\\Users\\gualt\\Desktop\\New devboule", seam());
    if (menu === null) throw new Error("menu was null");
    const { root } = await openMenu(menu);
    const rows = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    const copy = rows.find((row) => row.textContent === "Copy path");
    if (copy === undefined) throw new Error("Copy path did not render");
    await act(async () => {
      copy.click();
    });
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith("C:\\Users\\gualt\\Desktop\\New devboule");
    expect(copy.textContent).toBe("Copied");
    await act(async () => root.unmount());
  });

  it("shows the path middle-truncated with the full path in a tooltip", async () => {
    const long = `C:\\Users\\gualt\\Desktop\\${"nested\\".repeat(10)}work`;
    const menu = headerMenu(long, seam());
    if (menu === null) throw new Error("menu was null");
    const { root } = await openMenu(menu);
    const note = document.querySelector(".pane-header-path");
    if (note === null) throw new Error("path note did not render");
    expect(note.getAttribute("title")).toBe(long);
    expect(note.textContent ?? "").not.toBe(long);
    expect(note.textContent).toContain("…");
    await act(async () => root.unmount());
  });
});

describe("pane header menu keyboard", () => {
  it("opens on the first entry, arrows move, Escape returns focus to the kebab", async () => {
    const menu = headerMenu("C:\\x", seam());
    if (menu === null) throw new Error("menu was null");
    const { root, kebab } = await openMenu(menu);
    const rows = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    const enabled = rows.filter((row) => !row.disabled);
    expect(document.activeElement).toBe(enabled[0]);
    await act(async () => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      );
    });
    expect(document.activeElement).toBe(enabled[1]);
    await act(async () => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }),
      );
    });
    expect(document.activeElement).toBe(enabled[0]);
    await act(async () => {
      document.activeElement!.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(document.querySelector("[role='menu']")).toBeNull();
    expect(document.activeElement).toBe(kebab);
    await act(async () => root.unmount());
  });
});

describe("middleTruncate", () => {
  it("leaves a short path whole", () => {
    expect(middleTruncate("C:\\x", 28)).toBe("C:\\x");
  });

  it("keeps the head and the tail with one ellipsis", () => {
    const long = `C:\\Users\\gualt\\Desktop\\${"nested\\".repeat(10)}work`;
    const cut = middleTruncate(long, 28);
    expect(cut).toContain("…");
    expect(cut.startsWith("C:\\Users")).toBe(true);
    expect(cut.endsWith("work")).toBe(true);
    expect([...cut].length).toBeLessThanOrEqual(29);
  });

  it("never splits an astral character", () => {
    const long = `C:\\${"🗂️".repeat(40)}\\tail`;
    const cut = middleTruncate(long, 20);
    expect(cut).toContain("…");
    expect(cut.endsWith("tail")).toBe(true);
  });
});
