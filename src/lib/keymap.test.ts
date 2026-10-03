// @vitest-environment happy-dom

import { afterEach, describe, expect, it, vi } from "vitest";
import {
  browserFocusAddress,
  browserReloadChord,
  COMMAND_MENU_KEY,
  commandModifier,
  commandModifierLabel,
  composerChordLabel,
  composerKeyAction,
  crescentPageForKey,
  isCloseTabKey,
  searchChordFor,
  searchChordLabel,
  shortcutSections,
  stripChordFor,
  tabMoveForKey,
  type ComposerKeyAction,
} from "./keymap";

type StripChordEvent = Parameters<typeof stripChordFor>[0];

interface ComposerEventFixture {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  isComposing: boolean;
  keyCode: number;
}

function chord(overrides: Partial<StripChordEvent> = {}): StripChordEvent {
  return {
    key: "]",
    altKey: true,
    shiftKey: true,
    ctrlKey: false,
    metaKey: false,
    isComposing: false,
    keyCode: 0,
    target: null,
    ...overrides,
  };
}

function enter(overrides: Partial<ComposerEventFixture> = {}): ComposerEventFixture {
  return {
    key: "Enter",
    shiftKey: false,
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    isComposing: false,
    keyCode: 0,
    ...overrides,
  };
}

function rows(behavior: "queue" | "interrupt-and-send"): string {
  return shortcutSections(behavior)
    .flatMap((section) => section.rows)
    .map((row) => `${row.keys} | ${row.title} | ${row.detail ?? ""}`)
    .join("\n");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("tabMoveForKey", () => {
  it.each([
    ["ArrowRight", "next"],
    ["ArrowDown", "next"],
    ["ArrowLeft", "previous"],
    ["ArrowUp", "previous"],
    ["Home", "first"],
    ["End", "last"],
  ] as const)("reads %s as %s", (key, move) => {
    expect(tabMoveForKey(key)).toBe(move);
  });

  it.each([["Tab"], ["Enter"], ["a"], ["arrowright"], [" "]])("leaves %s to the widget", (key) => {
    expect(tabMoveForKey(key)).toBeNull();
  });
});

describe("isCloseTabKey", () => {
  it("reads the two close keys", () => {
    expect(isCloseTabKey("Delete")).toBe(true);
    expect(isCloseTabKey("Backspace")).toBe(true);
  });

  it("leaves every other key alone", () => {
    expect(isCloseTabKey("d")).toBe(false);
    expect(isCloseTabKey("Escape")).toBe(false);
  });
});

describe("stripChordFor", () => {
  it("reads the two window-level chords", () => {
    expect(stripChordFor(chord({ key: "]" }))).toBe("next");
    expect(stripChordFor(chord({ key: "[" }))).toBe("previous");
  });

  it.each([
    ["no Alt", { altKey: false }],
    ["no Shift", { shiftKey: false }],
    ["Ctrl", { ctrlKey: true }],
    ["Meta", { metaKey: true }],
  ] as const)("rejects the chord with %s", (_label, overrides) => {
    expect(stripChordFor(chord(overrides))).toBeNull();
  });

  it.each([["p"], ["}"], ["{"], ["ArrowRight"]])("rejects the %s key", (key) => {
    expect(stripChordFor(chord({ key }))).toBeNull();
  });

  it("rejects a composition, both signals", () => {
    expect(stripChordFor(chord({ isComposing: true }))).toBeNull();
    expect(stripChordFor(chord({ keyCode: 229 }))).toBeNull();
  });

  it.each([["input"], ["textarea"], ["select"]])("rejects a focused %s field", (tag) => {
    expect(stripChordFor(chord({ target: document.createElement(tag) }))).toBeNull();
  });

  it("rejects an editable region and a terminal", () => {
    const editable = document.createElement("div");
    editable.contentEditable = "true";
    expect(stripChordFor(chord({ target: editable }))).toBeNull();

    const terminal = document.createElement("div");
    terminal.className = "workspace-terminal-shell";
    const inside = document.createElement("div");
    terminal.appendChild(inside);
    expect(stripChordFor(chord({ target: inside }))).toBeNull();
  });

  it.each([["menu"], ["dialog"], ["alertdialog"], ["listbox"]])("rejects an open %s", (role) => {
    const dialog = document.createElement("div");
    dialog.setAttribute("role", role);
    const inside = document.createElement("span");
    dialog.appendChild(inside);
    expect(stripChordFor(chord({ target: inside }))).toBeNull();
  });

  it("accepts a focused plain element and a missing target", () => {
    expect(stripChordFor(chord({ target: document.createElement("div") }))).toBe("next");
    expect(stripChordFor(chord({ target: document.body }))).toBe("next");
  });
});

describe("composerKeyAction", () => {
  it("reads Enter as the submit, with Alt riding along", () => {
    expect(composerKeyAction(enter())).toBe("submit");
    expect(composerKeyAction(enter({ altKey: true }))).toBe("submit");
  });

  it("reads the two command modifiers as the alternate action", () => {
    expect(composerKeyAction(enter({ ctrlKey: true }))).toBe("alternate");
    expect(composerKeyAction(enter({ metaKey: true }))).toBe("alternate");
  });

  it("reads Shift as the newline, and Shift wins over a command modifier", () => {
    expect(composerKeyAction(enter({ shiftKey: true }))).toBe("newline");
    expect(composerKeyAction(enter({ shiftKey: true, ctrlKey: true }))).toBe("newline");
  });

  it("leaves other keys and a composition to the field", () => {
    expect(composerKeyAction(enter({ key: "Escape" }))).toBeNull();
    expect(composerKeyAction(enter({ key: "a" }))).toBeNull();
    expect(composerKeyAction(enter({ isComposing: true }))).toBeNull();
    expect(composerKeyAction(enter({ keyCode: 229 }))).toBeNull();
  });
});

describe("crescentPageForKey", () => {
  it("reads the two paging keys", () => {
    expect(crescentPageForKey("ArrowRight")).toBe("next");
    expect(crescentPageForKey("ArrowLeft")).toBe("previous");
  });

  it("leaves the other keys alone", () => {
    expect(crescentPageForKey("ArrowUp")).toBeNull();
    expect(crescentPageForKey("Escape")).toBeNull();
  });
});

describe("the command modifier display", () => {
  it("names Cmd on macOS and Ctrl everywhere else", () => {
    expect(commandModifierLabel("MacIntel")).toBe("Cmd");
    expect(commandModifierLabel("MacARM64")).toBe("Cmd");
    expect(commandModifierLabel("Win32")).toBe("Ctrl");
    expect(commandModifierLabel("Linux x86_64")).toBe("Ctrl");
    expect(commandModifierLabel("")).toBe("Ctrl");
  });

  it("reads the host platform when the page asks", () => {
    vi.stubGlobal("navigator", { platform: "MacIntel" });
    expect(commandModifier()).toBe("Cmd");
    vi.stubGlobal("navigator", { platform: "Win32" });
    expect(commandModifier()).toBe("Ctrl");
  });

  it("prints the composer chords from the same actions the matcher reads", () => {
    const actions: readonly ComposerKeyAction[] = ["submit", "alternate", "newline"];
    expect(actions.map(composerChordLabel)).toEqual([
      "Enter",
      `${commandModifier()}+Enter`,
      "Shift+Enter",
    ]);
  });
});

describe("shortcutSections", () => {
  it("groups the rows under the five scopes, in order", () => {
    expect(shortcutSections("queue").map((section) => section.label)).toEqual([
      "Tabs",
      "Composer",
      "Navigation",
      "Panel",
      "Browser",
    ]);
  });

  it("lists the keys the matchers answer", () => {
    const listed = rows("queue");
    expect(listed).toContain("Alt+Shift+]");
    expect(listed).toContain("Alt+Shift+[");
    expect(listed).toContain("ArrowRight / ArrowDown");
    expect(listed).toContain("ArrowLeft / ArrowUp");
    expect(listed).toContain("Home / End");
    expect(listed).toContain("Delete / Backspace");
    expect(listed).toContain(COMMAND_MENU_KEY);

    expect(stripChordFor(chord({ key: "]" }))).toBe("next");
    expect(tabMoveForKey("ArrowDown")).toBe("next");
    expect(tabMoveForKey("Home")).toBe("first");
    expect(crescentPageForKey("ArrowLeft")).toBe("previous");
    expect(isCloseTabKey("Backspace")).toBe(true);
  });

  it("lists the search chord the sidebar answers", () => {
    expect(rows("queue")).toContain(`${searchChordLabel()} | Search workspaces |`);
    expect(searchChordFor(chord({ key: "k", altKey: false, shiftKey: false, ctrlKey: true }))).toBe(
      true,
    );
  });

  it("shows the host's own command modifier in the alternate row", () => {
    vi.stubGlobal("navigator", { platform: "MacIntel" });
    expect(rows("queue")).toContain("Cmd+Enter");
    vi.stubGlobal("navigator", { platform: "Win32" });
    expect(rows("queue")).toContain("Ctrl+Enter");
  });

  it("scopes the tab chord and the crescent arrows to what really happens", () => {
    const listed = rows("queue");
    expect(listed).toContain(
      "Not while typing in a field or terminal, or while a menu, list or dialog is open.",
    );
    expect(listed).toContain("While the surface list is open, outside text fields and dialogs.");
  });
});

describe("the browser chords", () => {
  const chord = (key: string, over: Partial<KeyboardEvent> = {}) => ({
    key,
    ctrlKey: true,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    ...over,
  });

  it("answers the command modifier with L and with R, either modifier", () => {
    expect(browserFocusAddress(chord("l"))).toBe(true);
    expect(browserReloadChord(chord("r"))).toBe(true);
    expect(browserFocusAddress(chord("L"))).toBe(true);
    expect(browserFocusAddress(chord("l", { ctrlKey: false, metaKey: true }))).toBe(true);
    expect(browserReloadChord(chord("r", { ctrlKey: false, metaKey: true }))).toBe(true);
  });

  it("leaves every other chord to whatever else owns it", () => {
    expect(browserFocusAddress(chord("k"))).toBe(false);
    expect(browserReloadChord(chord("f"))).toBe(false);
    // Alt and Shift make it a different chord: reload is not force-reload and
    // selecting an address is not a select-all.
    expect(browserFocusAddress(chord("l", { shiftKey: true }))).toBe(false);
    expect(browserReloadChord(chord("r", { altKey: true }))).toBe(false);
    // An unmodified letter is text.
    expect(browserFocusAddress({ ...chord("l"), ctrlKey: false })).toBe(false);
  });

  it("lists both chords on the Shortcuts page", () => {
    vi.stubGlobal("navigator", { platform: "Win32" });
    expect(rows("queue")).toContain("Ctrl+L");
    expect(rows("queue")).toContain("Ctrl+R");
  });
});
