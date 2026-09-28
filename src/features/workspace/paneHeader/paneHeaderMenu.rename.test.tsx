// @vitest-environment happy-dom

// The pane header's kebab Rename entry and the dialog it opens: the entry's
// presence by the seam, its order, and the focus handoff to the real dialog.
// Split from paneHeaderMenu.test.tsx — a fourth topic in one file is three too
// many, and this one carries its own dialog integration.

import { act, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return { ...actual, sessionSetName: invokeMock };
});

import { sessionSetName } from "../../../lib/tauri";
import { buildTabCloseEntries } from "../strip/tabCloseMenu";
import { SessionRenameDialog } from "../strip/SessionRenameDialog";
import { PaneHeader } from "./PaneHeader";
import { headerMenu, type PaneHeaderMenu } from "./paneHeaderMenu";

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

function seam() {
  return { closeEntries: buildTabCloseEntries(1, 3), onCloseEntry: vi.fn() };
}

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

describe("pane header menu rename", () => {
  it("hides Rename when the seam carries no onRename — the daemon's gate", async () => {
    const menu = headerMenu("C:\\x", seam());
    if (menu === null) throw new Error("menu was null");
    const { root } = await openMenu(menu);
    const rows = [...document.querySelectorAll("[role='menuitem']")].map((row) => row.textContent);
    expect(rows).not.toContain("Rename");
    await act(async () => root.unmount());
  });

  it("shows Rename after Copy path when the seam carries onRename", async () => {
    const menu = headerMenu("C:\\x", { ...seam(), onRename: vi.fn() });
    if (menu === null) throw new Error("menu was null");
    const { root } = await openMenu(menu);
    const rows = [...document.querySelectorAll("[role='menuitem']")].map((row) => row.textContent);
    expect(rows).toEqual([
      "Copy path",
      "Rename",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);
    await act(async () => root.unmount());
  });

  it("keeps a Rename-only menu actionable — no path and no close entries", () => {
    const menu = headerMenu(undefined, {
      closeEntries: [],
      onCloseEntry: vi.fn(),
      onRename: vi.fn(),
    });
    expect(menu).not.toBeNull();
  });

  it("fires onRename and closes the menu", async () => {
    const onRename = vi.fn();
    const menu = headerMenu("C:\\x", { ...seam(), onRename });
    if (menu === null) throw new Error("menu was null");
    const { root } = await openMenu(menu);
    const rename = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (row) => row.textContent === "Rename",
    );
    if (rename === undefined) throw new Error("Rename did not render");
    await act(async () => {
      rename.click();
    });
    expect(onRename).toHaveBeenCalledTimes(1);
    expect(document.querySelector("[role='menu']")).toBeNull();
    await act(async () => root.unmount());
  });

  it("returns focus to the kebab after the dialog saves", async () => {
    const { host, root, kebab } = await openHeaderWithDialog();
    await act(async () => {
      kebab.click();
    });
    const rename = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (row) => row.textContent === "Rename",
    );
    if (rename === undefined) throw new Error("Rename did not render");
    await act(async () => {
      rename.click();
    });
    const field = document.querySelector<HTMLInputElement>('[role="dialog"] input');
    if (field === null) throw new Error("rename dialog did not open");
    expect(document.activeElement).toBe(field);
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("input value setter did not exist");
    await act(async () => {
      setValue.call(field, "worker two");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
    await act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
      );
    });
    expect(sessionSetName).toHaveBeenCalledWith("s.4242.7", "worker two");
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(kebab);
    await act(async () => root.unmount());
    host.remove();
  });

  it("returns focus to the kebab after the dialog is cancelled", async () => {
    const { host, root, kebab } = await openHeaderWithDialog();
    await act(async () => {
      kebab.click();
    });
    const rename = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (row) => row.textContent === "Rename",
    );
    if (rename === undefined) throw new Error("Rename did not render");
    await act(async () => {
      rename.click();
    });
    await act(async () => {
      document
        .querySelector<HTMLElement>('[role="dialog"] input')
        ?.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
        );
    });
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(kebab);
    await act(async () => root.unmount());
    host.remove();
  });
});

/** The whole path: the real kebab wired to the real dialog, the seam's
 * onRename opening it the way Workspace wires the capability-gated seam. */
async function openHeaderWithDialog() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  function Probe() {
    const [rename, setRename] = useState<{ sessionId: string; title: string } | null>(null);
    const menu = headerMenu("C:\\x", {
      closeEntries: buildTabCloseEntries(1, 3),
      onCloseEntry: () => undefined,
      onRename: () => setRename({ sessionId: "s.4242.7", title: "worker one" }),
    });
    return (
      <>
        <PaneHeader kind="agent" title="worker one" display={DISPLAY} menu={menu} />
        <SessionRenameDialog rename={rename} onClose={() => setRename(null)} />
      </>
    );
  }
  await act(async () => {
    root.render(<Probe />);
  });
  const kebab = host.querySelector<HTMLButtonElement>(".pane-header-kebab");
  if (kebab === null) throw new Error("kebab did not render");
  return { host, root, kebab };
}
