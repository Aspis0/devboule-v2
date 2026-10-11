// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { panelChordFor, type PanelChord } from "./panelChords";

interface ChordInput {
  key: string;
  ctrl?: boolean;
  meta?: boolean;
  alt?: boolean;
  shift?: boolean;
  composing?: boolean;
  repeat?: boolean;
}

function chord(input: ChordInput, target: EventTarget | null = null) {
  return {
    key: input.key,
    ctrlKey: input.ctrl ?? false,
    metaKey: input.meta ?? false,
    altKey: input.alt ?? false,
    shiftKey: input.shift ?? false,
    isComposing: input.composing ?? false,
    repeat: input.repeat ?? false,
    keyCode: 0,
    target,
  };
}

function terminalTarget(): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = '<div class="workspace-terminal-shell"><textarea></textarea></div>';
  document.body.append(host);
  return host.querySelector("textarea") as HTMLElement;
}

function fieldTarget(): HTMLElement {
  const host = document.createElement("div");
  host.innerHTML = "<input />";
  document.body.append(host);
  return host.querySelector("input") as HTMLElement;
}

describe("the panel toggle chords on Windows and Linux", () => {
  it.each<[string, ChordInput, PanelChord | null]>([
    ["Ctrl+B", { key: "b", ctrl: true }, "sidebar"],
    ["Ctrl+E", { key: "e", ctrl: true }, "panel"],
    ["Ctrl+.", { key: ".", ctrl: true }, "both"],
    ["Ctrl+Shift+B", { key: "B", ctrl: true, shift: true }, null],
    ["Ctrl+Alt+B", { key: "b", ctrl: true, alt: true }, null],
    ["Ctrl+X", { key: "x", ctrl: true }, null],
    ["Meta+B", { key: "b", meta: true }, null],
    ["Ctrl+B mid-composition", { key: "b", ctrl: true, composing: true }, null],
  ])("answers %s outside a terminal", (_name, input, want) => {
    expect(panelChordFor(chord(input), false)).toBe(want);
  });

  it.each<[string, ChordInput]>([
    ["Ctrl+B", { key: "b", ctrl: true }],
    ["Ctrl+E", { key: "e", ctrl: true }],
    ["Ctrl+.", { key: ".", ctrl: true }],
  ])("stands %s down in a focused terminal, so the shell keeps it", (_name, input) => {
    expect(panelChordFor(chord(input, terminalTarget()), false)).toBeNull();
  });

  it("fires from a text field, which is no reason to stand down", () => {
    expect(panelChordFor(chord({ key: "b", ctrl: true }, fieldTarget()), false)).toBe("sidebar");
  });
});

describe("the panel toggle chords on macOS", () => {
  it.each<[string, ChordInput, PanelChord | null]>([
    ["⌘B", { key: "b", meta: true }, "sidebar"],
    ["⌘E", { key: "e", meta: true }, "panel"],
    ["⌘.", { key: ".", meta: true }, "both"],
    ["Ctrl+B", { key: "b", ctrl: true }, null],
    ["⌘Shift+B", { key: "B", meta: true, shift: true }, null],
  ])("answers %s outside a terminal", (_name, input, want) => {
    expect(panelChordFor(chord(input), true)).toBe(want);
  });

  it.each<[string, ChordInput]>([
    ["⌘B", { key: "b", meta: true }],
    ["⌘E", { key: "e", meta: true }],
    ["⌘.", { key: ".", meta: true }],
  ])("stands %s down in a focused terminal too", (_name, input) => {
    expect(panelChordFor(chord(input, terminalTarget()), true)).toBeNull();
  });
});

describe("the panel toggle chords on a held key", () => {
  it("answers only the press, not the auto-repeat that a held chord sends", () => {
    expect(panelChordFor(chord({ key: "b", ctrl: true, repeat: true }), false)).toBeNull();
    expect(panelChordFor(chord({ key: "b", meta: true, repeat: true }), true)).toBeNull();
  });
});
