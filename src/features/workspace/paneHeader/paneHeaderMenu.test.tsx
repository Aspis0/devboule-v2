// @vitest-environment happy-dom

// The pane header's kebab: entries limited to actions with a working path
// (copy the path when known; the tab menu's close group), keyboard that
// follows the strip's tab menu, and focus that returns to the kebab.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it, vi } from "vitest";
import { buildTabCloseEntries } from "../strip/tabCloseMenu";
import { PaneHeader } from "./PaneHeader";
import {
  agentHeaderMenu,
  middleTruncate,
  terminalHeaderMenu,
  type PaneHeaderMenu,
} from "./paneHeaderMenu";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function renderHeader() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  return { host, root };
}

async function openMenu(menu: PaneHeaderMenu) {
  const { host, root } = renderHeader();
  await act(async () => {
    root.render(
      <PaneHeader
        kind="agent"
        title="Claude"
        display={{ word: "Running", tone: "green", pulse: true, tooltip: "Running" }}
        menu={menu}
      />,
    );
  });
  const kebab = host.querySelector<HTMLButtonElement>(".pane-header-kebab");
  if (kebab === null) throw new Error("kebab did not render");
  await act(async () => {
    kebab.click();
  });
  return { host, root, kebab };
}

describe("pane header menu entries", () => {
  it("holds Copy path plus the tab menu's close group, and nothing else", async () => {
    const menu = agentHeaderMenu("C:\\Users\\gualt\\Desktop", {
      closeEntries: buildTabCloseEntries(1, 3),
      onCloseEntry: () => undefined,
    });
    if (menu === null) throw new Error("agent menu was null");
    const { host, root } = await openMenu(menu);
    const rows = [...host.querySelectorAll("[role='menuitem']")].map((row) => row.textContent);
    expect(rows).toEqual(["Copy path", "Close to the right", "Close other tabs", "Close"]);
    expect(host.textContent).not.toContain("Rename");
    expect(host.textContent).not.toContain("Delete");
    expect(host.textContent).not.toContain("Close to the left");
    await act(async () => root.unmount());
  });

  it("reuses the tab menu's labels verbatim", () => {
    const menu = agentHeaderMenu("C:\\x", {
      closeEntries: buildTabCloseEntries(1, 3),
      onCloseEntry: () => undefined,
    });
    if (menu === null) throw new Error("agent menu was null");
    const tabLabels = new Map(buildTabCloseEntries(1, 3).map((entry) => [entry.key, entry.label]));
    for (const entry of menu.closeEntries) {
      expect(entry.label).toBe(tabLabels.get(entry.key));
    }
  });

  it("renders no kebab when nothing can act", () => {
    expect(agentHeaderMenu(undefined, undefined)).toBeNull();
    expect(terminalHeaderMenu(undefined, undefined, undefined)).toBeNull();
  });

  it("leaves the close group disabled until the workspace wires it", async () => {
    const menu = agentHeaderMenu("C:\\x", undefined);
    if (menu === null) throw new Error("agent menu was null");
    const { host, root } = await openMenu(menu);
    const rows = [...host.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    expect(rows[0]?.disabled).toBe(false);
    expect(rows.slice(1).map((row) => row.disabled)).toEqual([true, true, true]);
    await act(async () => root.unmount());
  });

  it("enables this tab's Close through the terminal's existing close path", async () => {
    const onCloseTab = vi.fn();
    const menu = terminalHeaderMenu("C:\\x", onCloseTab, undefined);
    if (menu === null) throw new Error("terminal menu was null");
    const { host, root } = await openMenu(menu);
    const rows = [...host.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    const close = rows.find((row) => row.textContent === "Close");
    if (close === undefined) throw new Error("Close entry did not render");
    expect(close.disabled).toBe(false);
    await act(async () => {
      close.click();
    });
    expect(onCloseTab).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
  });

  it("dispatches a wired close entry with its tab-menu key", async () => {
    const onCloseEntry = vi.fn();
    const menu = agentHeaderMenu("C:\\x", {
      closeEntries: buildTabCloseEntries(1, 3),
      onCloseEntry,
    });
    if (menu === null) throw new Error("agent menu was null");
    const { host, root } = await openMenu(menu);
    const rows = [...host.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    const others = rows.find((row) => row.textContent === "Close other tabs");
    if (others === undefined) throw new Error("Close other tabs did not render");
    await act(async () => {
      others.click();
    });
    expect(onCloseEntry).toHaveBeenCalledTimes(1);
    expect(onCloseEntry).toHaveBeenCalledWith("others");
    await act(async () => root.unmount());
  });
});

describe("pane header menu copy", () => {
  it("copies the full path and says so on the row", async () => {
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const menu = agentHeaderMenu("C:\\Users\\gualt\\Desktop\\New devboule", undefined);
    if (menu === null) throw new Error("agent menu was null");
    const { host, root } = await openMenu(menu);
    const rows = [...host.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
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
    const menu = agentHeaderMenu(long, undefined);
    if (menu === null) throw new Error("agent menu was null");
    const { host, root } = await openMenu(menu);
    const note = host.querySelector(".pane-header-path");
    if (note === null) throw new Error("path note did not render");
    expect(note.getAttribute("title")).toBe(long);
    expect(note.textContent ?? "").not.toBe(long);
    expect(note.textContent).toContain("…");
    await act(async () => root.unmount());
  });
});

describe("pane header menu keyboard", () => {
  it("opens on the first enabled entry, arrows move, Escape returns focus to the kebab", async () => {
    const menu = agentHeaderMenu("C:\\x", undefined);
    if (menu === null) throw new Error("agent menu was null");
    const { host, root, kebab } = await openMenu(menu);
    const rows = [...host.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    expect(document.activeElement).toBe(rows[0]);
    await act(async () => {
      rows[0]!.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    });
    // The close group is disabled without the workspace seam, so arrows wrap
    // on the one enabled row instead of landing on a dead entry.
    expect(document.activeElement).toBe(rows[0]);
    await act(async () => {
      rows[0]!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(host.querySelector("[role='menu']")).toBeNull();
    expect(document.activeElement).toBe(kebab);
    await act(async () => root.unmount());
  });

  it("moves across enabled entries when the workspace wired them", async () => {
    const menu = agentHeaderMenu("C:\\x", {
      closeEntries: buildTabCloseEntries(1, 3),
      onCloseEntry: () => undefined,
    });
    if (menu === null) throw new Error("agent menu was null");
    const { host, root } = await openMenu(menu);
    const rows = [...host.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
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
    await act(async () => root.unmount());
  });
});

describe("middleTruncate", () => {
  it("leaves a short path whole", () => {
    expect(middleTruncate("C:\\x", 48)).toBe("C:\\x");
  });

  it("keeps the head and the tail with one ellipsis", () => {
    const long = `C:\\Users\\gualt\\Desktop\\${"nested\\".repeat(10)}work`;
    const cut = middleTruncate(long, 48);
    expect(cut).toContain("…");
    expect(cut.startsWith("C:\\Users")).toBe(true);
    expect(cut.endsWith("work")).toBe(true);
    expect([...cut].length).toBeLessThanOrEqual(49);
  });

  it("never splits an astral character", () => {
    const long = `C:\\${"🗂️".repeat(40)}\\tail`;
    const cut = middleTruncate(long, 20);
    expect(cut).toContain("…");
    expect(cut.endsWith("tail")).toBe(true);
  });
});
