// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { buildTabCloseEntries } from "../strip/tabCloseMenu";
import { PaneHeader } from "./PaneHeader";
import { headerMenu } from "./paneHeaderMenu";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const writeText = vi.fn(async (_value: string): Promise<void> => undefined);
const onCloseEntry = vi.fn();
let root: Root | null = null;

beforeEach(() => {
  vi.useFakeTimers();
  writeText.mockReset().mockResolvedValue(undefined);
  onCloseEntry.mockClear();
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  document.body.replaceChildren();
  vi.useRealTimers();
  Reflect.deleteProperty(navigator, "clipboard");
});

async function open(
  kind: "agent" | "terminal",
  cwd?: string,
  closeEntries = buildTabCloseEntries(1, 3),
) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  const menu = headerMenu(cwd, { closeEntries, onCloseEntry }, "stable-id");
  await act(async () =>
    root!.render(
      <PaneHeader
        kind={kind}
        title="Human title"
        menu={menu}
        display={{
          word: "Running",
          detail: null,
          tone: "green",
          pulse: true,
          tooltip: "Running",
          srDetail: null,
        }}
      />,
    ),
  );
  await act(async () => host.querySelector<HTMLButtonElement>(".pane-header-kebab")!.click());
}

function item(label: string): HTMLButtonElement {
  const button = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(
    (row) => row.textContent === label,
  );
  if (button === undefined) throw new Error(`Missing menu item: ${label}`);
  return button;
}

describe("pane menu copies", () => {
  it.each(["agent", "terminal"] as const)("copies the stable ID of a %s pane", async (kind) => {
    await open(kind);
    expect(
      [...document.querySelectorAll('[role="menuitem"]')].map((row) => row.textContent),
    ).toEqual(["Copy session ID", "Close to the right", "Close other tabs", "Close"]);
    expect(document.activeElement).toBe(item("Copy session ID"));
    await act(async () => item("Copy session ID").click());
    expect(writeText).toHaveBeenCalledExactlyOnceWith("stable-id");
    expect(item("Copied")).not.toBeNull();
    const status = document.querySelector('.pane-header-menu [role="status"]');
    expect(status?.textContent).toBe("Session ID copied");
    expect(status?.previousElementSibling).toBe(
      document.querySelector('.pane-header-menu [role="menu"]'),
    );
    expect(status?.closest('[role="menu"]')).toBeNull();
    expect(onCloseEntry).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTimeAsync(1500));
    expect(item("Copy session ID")).not.toBeNull();
  });

  it.each([
    [
      String.raw`\\?\C:\working directory\long project name`,
      String.raw`C:\working directory\long project name`,
    ],
    [String.raw`\\?\UNC\server\share\project`, String.raw`\\server\share\project`],
  ])("displays and copies the human path for %s", async (cwd, expected) => {
    await open("agent", cwd);
    expect(document.querySelector(".pane-header-path")?.getAttribute("title")).toBe(expected);
    const copyPath = item("Copy path");
    expect(copyPath.nextElementSibling?.getAttribute("role")).toBe("separator");
    await act(async () => copyPath.click());
    expect(writeText).toHaveBeenCalledExactlyOnceWith(expected);
    expect(item("Copy session ID")).not.toBeNull();
    expect(copyPath.textContent).toBe("Copied");
    expect(document.querySelector('.pane-header-menu [role="status"]')?.textContent).toBe(
      "Path copied",
    );
    expect(onCloseEntry).not.toHaveBeenCalled();
  });

  it.each([
    ["rejected", "Copy session ID", "Session ID copy failed"],
    ["unavailable", "Copy session ID", "Session ID copy failed"],
    ["rejected", "Copy path", "Path copy failed"],
    ["unavailable", "Copy path", "Path copy failed"],
  ])("reports a %s clipboard failure for %s", async (kind, label, announcement) => {
    if (kind === "rejected") writeText.mockRejectedValueOnce(new Error("denied"));
    else Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    await open("terminal", "C:/project");
    await act(async () => item(label).click());
    expect(item("Copy failed")).not.toBeNull();
    expect(document.querySelector('.pane-header-menu [role="status"]')?.textContent).toBe(
      announcement,
    );
    expect(onCloseEntry).not.toHaveBeenCalled();
  });

  it("omits the separator when copy rows have no close group", async () => {
    await open("agent", "C:/p", []);
    expect(item("Copy path")).not.toBeNull();
    expect(document.querySelector('[role="separator"]')).toBeNull();
  });

  it("keeps feedback on the most recently activated copy row", async () => {
    let completeId: (() => void) | undefined;
    writeText.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          completeId = resolve;
        }),
    );
    await open("agent", "C:/project");
    await act(async () => item("Copy session ID").click());
    await act(async () => item("Copy path").click());
    await act(async () => completeId!());
    expect(item("Copy session ID")).not.toBeNull();
    expect(item("Copied").previousElementSibling).toBe(item("Copy session ID"));
    expect(writeText.mock.calls).toEqual([["stable-id"], ["C:/project"]]);
  });
});
