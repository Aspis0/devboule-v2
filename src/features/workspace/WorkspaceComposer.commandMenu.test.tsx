// The composer's "/" menu as a widget: the rows the arrows walk, what Enter and
// Tab put into the text, what Escape, the pointer and the modifiers leave alone,
// the scroll the menu owns, and the states it shows when the daemon has nothing
// (or nothing matching) to show. What Enter sends or queues is
// `WorkspaceComposer.sendKeys.test.tsx`; the surface-level menu is asserted in
// `AgentChatSurface.test.tsx`.
// @vitest-environment happy-dom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import {
  composerDrivers,
  composerProps,
  MENU_COMMANDS,
  type ComposerDrivers,
  type ComposerMocks,
} from "./composerTestKit";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** The geometry happy-dom does not lay out: rows at content offsets, a box that
 * shows a window of them. Values the scroll maths reads, nothing more. */
function stubGeometry(element: Element, offsetTop: number, offsetHeight: number): void {
  Object.defineProperty(element, "offsetTop", { value: offsetTop, configurable: true });
  Object.defineProperty(element, "offsetHeight", { value: offsetHeight, configurable: true });
}

let container: HTMLDivElement;
let root: Root;
let onSend: Mock<(text: string) => void>;
let onQueue: Mock<(text: string) => void>;
let mocks: ComposerMocks;
let drive: ComposerDrivers;

async function renderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(mocks, overrides)} />);
  });
}

async function rerenderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(mocks, overrides)} />);
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  onSend = vi.fn<(text: string) => void>();
  onQueue = vi.fn<(text: string) => void>();
  mocks = { onSend, onQueue };
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("the open menu's keys", () => {
  it("starts on the first match and walks the rows with the arrows, wrapping at both ends", async () => {
    await renderComposer();
    await drive.type("/");
    expect(drive.activeRow()).toBe(drive.rows()[0]);

    await drive.press("ArrowDown");
    expect(drive.activeRow()).toBe(drive.rows()[1]);
    await drive.press("ArrowDown");
    expect(drive.activeRow()).toBe(drive.rows()[2]);
    await drive.press("ArrowDown");
    expect(drive.activeRow()).toBe(drive.rows()[0]);
    await drive.press("ArrowUp");
    expect(drive.activeRow()).toBe(drive.rows()[2]);
  });

  it("inserts the highlighted command on Enter instead of sending", async () => {
    await renderComposer();
    await drive.type("/");
    await drive.press("ArrowDown");
    await drive.press("Enter");

    expect(onSend).not.toHaveBeenCalled();
    expect(drive.textarea().value).toBe("/goal ");
    expect(drive.menu()).toBeNull();
  });

  it("takes Tab only while rows are shown, and leaves it to the browser everywhere else", async () => {
    await renderComposer();

    await drive.type("hello");
    const closedTab = await drive.press("Tab");
    expect(closedTab.defaultPrevented).toBe(false);

    // It completes once: the trailing space closes the menu, so the next Tab
    // is the browser's again and focus can leave the textarea.
    await drive.type("/");
    const openTab = await drive.press("Tab");
    expect(openTab.defaultPrevented).toBe(true);
    expect(drive.textarea().value).toBe("/build ");
    expect(drive.menu()).toBeNull();
    const afterInsert = await drive.press("Tab");
    expect(afterInsert.defaultPrevented).toBe(false);

    await drive.type("/zzz");
    expect(drive.rows()).toHaveLength(0);
    const noRows = await drive.press("Tab");
    expect(noRows.defaultPrevented).toBe(false);

    await drive.type("/go");
    await drive.press("Escape");
    const dismissed = await drive.press("Tab");
    expect(dismissed.defaultPrevented).toBe(false);
  });

  it("closes on Escape and keeps the text, with rows to pick and without", async () => {
    await renderComposer();
    await drive.type("/go");
    expect(drive.rows()).toHaveLength(1);

    await drive.press("Escape");

    expect(drive.menu()).toBeNull();
    expect(drive.textarea().value).toBe("/go");
    expect(onSend).not.toHaveBeenCalled();

    await drive.type("/zzz");
    expect(drive.rows()).toHaveLength(0);

    await drive.press("Escape");

    expect(drive.menu()).toBeNull();
    expect(drive.textarea().value).toBe("/zzz");
  });

  it("opens again on the next keystroke after Escape", async () => {
    await renderComposer();
    await drive.type("/w");
    await drive.press("Escape");
    expect(drive.menu()).toBeNull();

    await drive.type("/wo");

    expect(drive.rows()).toHaveLength(1);
    expect(drive.activeRow()).toBe(drive.rows()[0]);
  });

  it("refilters on typing and puts the highlight back on the first match", async () => {
    await renderComposer();
    await drive.type("/");
    await drive.press("ArrowDown");
    expect(drive.activeRow()).toBe(drive.rows()[1]);

    await drive.type("/o");

    expect(
      drive.rows().map((row) => row.querySelector(".workspace-command-name")?.textContent),
    ).toEqual(["/goal", "/workflow"]);
    expect(drive.activeRow()).toBe(drive.rows()[0]);
  });

  it("leaves the pointer's row out of the keys' row: Enter takes the highlight it drew", async () => {
    await renderComposer();
    await drive.type("/");

    await act(async () => {
      drive.rows()[2].dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    });
    expect(drive.activeRow()).toBe(drive.rows()[0]);
    expect(drive.rows()[2].getAttribute("aria-selected")).toBe("false");

    await drive.press("Enter");
    expect(drive.textarea().value).toBe("/build ");
  });

  it("scrolls only its own list, never the transcript above it", async () => {
    await renderComposer();
    await drive.type("/");
    const list = drive.menuOrFail();
    const rows = drive.rows();
    let scrollTop = 0;
    stubGeometry(rows[0], 0, 30);
    stubGeometry(rows[1], 200, 30);
    Object.defineProperty(list, "clientHeight", { value: 100, configurable: true });
    Object.defineProperty(list, "scrollTop", {
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
      configurable: true,
    });
    const scrollSpy = vi.spyOn(Element.prototype, "scrollIntoView");

    await drive.press("ArrowDown");
    // The row's bottom (230) is past the box's (100): the box scrolls, not an ancestor.
    expect(scrollTop).toBe(130);
    expect(scrollSpy).not.toHaveBeenCalled();

    await drive.press("ArrowUp");
    // The row is above the window (0 < 130): back to the top of the box.
    expect(scrollTop).toBe(0);
    scrollSpy.mockRestore();
  });

  it("keeps modified keys out of the menu, and lets Shift+Escape close it", async () => {
    await renderComposer();
    await drive.type("/");

    const jump = await drive.press("ArrowDown", { ctrlKey: true });
    expect(jump.defaultPrevented).toBe(false);
    expect(drive.activeRow()).toBe(drive.rows()[0]);

    const shiftEscape = await drive.press("Escape", { shiftKey: true });
    expect(shiftEscape.defaultPrevented).toBe(true);
    expect(drive.menu()).toBeNull();
    expect(drive.textarea().value).toBe("/");
  });

  it("puts the highlight back on the first match when the list shrinks under it, and keeps it there when the list returns", async () => {
    await renderComposer();
    await drive.type("/");
    await drive.press("ArrowDown");
    await drive.press("ArrowDown");
    expect(drive.activeRow()).toBe(drive.rows()[2]);

    await rerenderComposer({ availableCommands: MENU_COMMANDS.slice(0, 1) });

    expect(drive.rows()).toHaveLength(1);
    expect(drive.activeRow()).toBe(drive.rows()[0]);

    await rerenderComposer();

    // The stale row 2 must not come back with the list: the highlight is the
    // first match again, not the one that fell out of range.
    expect(drive.rows()).toHaveLength(MENU_COMMANDS.length);
    expect(drive.activeRow()).toBe(drive.rows()[0]);
  });

  it("sends on Enter when the open menu has no row to give", async () => {
    // Paseo hands the key back untouched when it has no option, so a menu
    // with nothing to complete never traps Enter.
    await renderComposer();
    await drive.type("/zzz");

    expect(drive.rows()).toHaveLength(0);
    await drive.press("Enter");

    expect(onSend).toHaveBeenCalledWith("/zzz");
    expect(drive.textarea().value).toBe("");
  });
});

describe("the menu with nothing to show", () => {
  it("shows a line instead of an empty box when no command was published", async () => {
    await renderComposer({ availableCommands: [] });
    await drive.type("/");

    expect(drive.menuOrFail().textContent).toContain("No commands found");
    expect(drive.rows()).toHaveLength(0);
    expect(drive.textarea().getAttribute("aria-activedescendant")).toBeNull();
  });

  it("shows the same line when the filter matches nothing", async () => {
    await renderComposer();
    await drive.type("/zzz");

    expect(drive.menuOrFail().textContent).toContain("No commands found");
    expect(drive.rows()).toHaveLength(0);
  });

  it("takes a list that arrives late into the menu that is already open", async () => {
    await renderComposer({ availableCommands: [] });
    await drive.type("/");
    expect(drive.menuOrFail().textContent).toContain("No commands found");

    await rerenderComposer();

    expect(drive.rows()).toHaveLength(MENU_COMMANDS.length);
    expect(drive.textarea().value).toBe("/");
    expect(drive.activeRow()).toBe(drive.rows()[0]);
  });

  it("names the highlighted row on a combobox that owns its list", async () => {
    await renderComposer();
    const box = drive.textarea();
    expect(box.getAttribute("role")).toBe("combobox");
    expect(box.getAttribute("aria-expanded")).toBe("false");
    expect(box.getAttribute("aria-activedescendant")).toBeNull();

    await drive.type("/");

    expect(box.getAttribute("aria-expanded")).toBe("true");
    const listId = box.getAttribute("aria-controls");
    const list = listId === null ? null : document.getElementById(listId);
    expect(list).toBe(drive.menuOrFail());
    expect(list?.getAttribute("role")).toBe("listbox");
    const first = drive.activeRow();
    expect(first?.getAttribute("role")).toBe("option");
    expect(first?.getAttribute("aria-selected")).toBe("true");
    expect(first?.closest('[role="listbox"]')).toBe(list);

    await drive.press("ArrowDown");

    expect(drive.activeRow()).toBe(drive.rows()[1]);
    expect(drive.rows()[0].getAttribute("aria-selected")).toBe("false");
    expect(drive.rows()[1].getAttribute("aria-selected")).toBe("true");
  });
});
