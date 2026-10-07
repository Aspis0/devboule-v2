// @vitest-environment happy-dom

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  clickMenuEntry,
  contextMenuKey,
  defaultSessions,
  menu,
  menuLabels,
  plainClick,
  renderWorkspace,
  rightClick,
  shiftF10,
  tabElement,
} from "./bulkCloseHarness";
import { sessionClose, sessionsList, sessionStop } from "../../lib/tauri";

const writeText = vi.fn(async (_value: string) => undefined);

beforeEach(() => {
  beforeEachHarness();
  writeText.mockReset().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
});

afterEach(async () => {
  await afterEachHarness();
  Reflect.deleteProperty(navigator, "clipboard");
});

describe("tab menu copy actions", () => {
  it.each(["agent-one", "session-2"])(
    "copies %s without closing its tab or session",
    async (id) => {
      await renderWorkspace();
      await rightClick(id);
      await clickMenuEntry("Copy session ID");
      expect(writeText).toHaveBeenCalledExactlyOnceWith(id);
      expect(menuLabels()).toContain("Copied");
      const status = menu().parentElement?.querySelector('[role="status"]');
      expect(status?.textContent).toBe("Session ID copied");
      expect(status?.previousElementSibling).toBe(menu());
      expect(status?.closest('[role="menu"]')).toBeNull();
      expect(sessionClose).not.toHaveBeenCalled();
      expect(sessionStop).not.toHaveBeenCalled();
      expect(tabElement(id)).not.toBeNull();
      await act(async () => vi.advanceTimersByTimeAsync(1500));
      expect(menuLabels()).toContain("Copy session ID");
    },
  );

  it.each([
    [String.raw`\\?\C:\my project\working directory`, String.raw`C:\my project\working directory`],
    [String.raw`\\?\UNC\server\share\my project`, String.raw`\\server\share\my project`],
  ])("copies the full display path for %s", async (cwd, expected) => {
    vi.mocked(sessionsList).mockResolvedValue(defaultSessions().map((row) => ({ ...row, cwd })));
    await renderWorkspace();
    await rightClick("agent-one");
    expect(menuLabels().slice(0, 2)).toEqual(["Copy session ID", "Copy path"]);
    const rows = [...menu().querySelectorAll('[role="menuitem"]')];
    expect(rows[2].textContent).toBe("Copy branch name");
    expect(rows[2].nextElementSibling?.getAttribute("role")).toBe("separator");
    await clickMenuEntry("Copy path");
    expect(writeText).toHaveBeenCalledExactlyOnceWith(expected);
    expect(menuLabels()).toContain("Copy session ID");
    expect(menuLabels()).toContain("Copied");
    expect(menu().parentElement?.querySelector('[role="status"]')?.textContent).toBe("Path copied");
  });

  it("omits Copy path when the session has no cwd", async () => {
    await renderWorkspace();
    await rightClick("session-2");
    expect(menuLabels()).not.toContain("Copy path");
    expect(menuLabels()).toContain("Copy branch name");
  });

  it.each([shiftF10, contextMenuKey])("opens copy actions from the keyboard", async (open) => {
    await renderWorkspace();
    await open("session-2");
    expect(document.activeElement?.textContent).toBe("Copy session ID");
    await act(async () => (document.activeElement as HTMLButtonElement).click());
    expect(writeText).toHaveBeenCalledExactlyOnceWith("session-2");
    await act(async () =>
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
      ),
    );
    expect(document.activeElement?.textContent).toBe("Copy branch name");
    await act(async () =>
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      ),
    );
    expect(document.activeElement).toBe(tabElement("session-2"));
  });

  it.each([
    ["rejected", "Copy session ID", "Session ID copy failed"],
    ["unavailable", "Copy session ID", "Session ID copy failed"],
    ["rejected", "Copy path", "Path copy failed"],
    ["unavailable", "Copy path", "Path copy failed"],
  ])("announces a %s clipboard failure for %s", async (kind, label, announcement) => {
    if (kind === "rejected") writeText.mockRejectedValueOnce(new Error("denied"));
    else Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    vi.mocked(sessionsList).mockResolvedValue(
      defaultSessions().map((row) => ({ ...row, cwd: "C:/project" })),
    );
    await renderWorkspace();
    await rightClick("session-2");
    await clickMenuEntry(label);
    expect(menuLabels()).toContain("Copy failed");
    expect(menu().parentElement?.querySelector('[role="status"]')?.textContent).toBe(announcement);
    expect(sessionClose).not.toHaveBeenCalled();
    expect(sessionStop).not.toHaveBeenCalled();
  });

  it.each(["session-2"])("copies the terminal %s from its pane menu", async (id) => {
    await renderWorkspace();
    await plainClick(id);
    const kebab = document.querySelector<HTMLButtonElement>(".pane-header-kebab");
    expect(kebab).not.toBeNull();
    await act(async () => kebab!.click());
    expect(menuLabels()).toContain("Copy session ID");
    await clickMenuEntry("Copy session ID");
    expect(writeText).toHaveBeenCalledExactlyOnceWith(id);
    expect(menu().parentElement?.querySelector('[role="status"]')?.textContent).toBe(
      "Session ID copied",
    );
    expect(sessionClose).not.toHaveBeenCalled();
    expect(sessionStop).not.toHaveBeenCalled();
  });

  it("resets feedback when another tab's menu opens", async () => {
    await renderWorkspace();
    await rightClick("agent-one");
    await clickMenuEntry("Copy session ID");
    await rightClick("session-2");
    expect(menuLabels()).toContain("Copy session ID");
    expect(menuLabels()).not.toContain("Copied");
  });
});
