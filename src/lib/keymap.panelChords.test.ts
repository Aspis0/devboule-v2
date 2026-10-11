// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { panelChordFor, type PanelChord } from "./keymap";

interface ChordInput {
  key: string;
  ctrl?: boolean;
  meta?: boolean;
  alt?: boolean;
  shift?: boolean;
  composing?: boolean;
}

function chord(input: ChordInput, target: EventTarget | null = null) {
  return {
    key: input.key,
    ctrlKey: input.ctrl ?? false,
    metaKey: input.meta ?? false,
    altKey: input.alt ?? false,
    shiftKey: input.shift ?? false,
    isComposing: input.composing ?? false,
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

describe("the panel toggle chords", () => {
  it.each<[string, ChordInput, PanelChord | null]>([
    ["Ctrl+B", { key: "b", ctrl: true }, "sidebar"],
    ["Ctrl+E", { key: "e", ctrl: true }, "panel"],
    ["Ctrl+.", { key: ".", ctrl: true }, "both"],
    ["Ctrl+Shift+B", { key: "B", ctrl: true, shift: true }, null],
    ["Ctrl+Alt+B", { key: "b", ctrl: true, alt: true }, null],
    ["Ctrl+X", { key: "x", ctrl: true }, null],
    ["Meta+B on Windows", { key: "b", meta: true }, null],
    ["Ctrl+B mid-composition", { key: "b", ctrl: true, composing: true }, null],
  ])("answers %s outside a terminal as the Windows and Linux command key", (_name, input, want) => {
    expect(panelChordFor(chord(input), false)).toBe(want);
  });

  it.each<[string, ChordInput, PanelChord | null]>([
    ["Ctrl+B", { key: "b", ctrl: true }, null],
    ["Ctrl+E", { key: "e", ctrl: true }, null],
    ["Ctrl+.", { key: ".", ctrl: true }, null],
  ])("lets %s stand down in a focused terminal, so the shell keeps it", (_name, input, want) => {
    expect(panelChordFor(chord(input, terminalTarget()), false)).toBe(want);
  });

  it.each<[string, ChordInput, PanelChord | null]>([
    ["⌘B", { key: "b", meta: true }, "sidebar"],
    ["⌘E", { key: "e", meta: true }, "panel"],
    ["⌘.", { key: ".", meta: true }, "both"],
    ["Ctrl+B on macOS", { key: "b", ctrl: true }, null],
    ["⌘Shift+B", { key: "B", meta: true, shift: true }, null],
  ])("answers %s on macOS", (_name, input, want) => {
    expect(panelChordFor(chord(input), true)).toBe(want);
  });

  it("still answers ⌘B inside a terminal, where ⌘ has no shell meaning", () => {
    expect(panelChordFor(chord({ key: "b", meta: true }, terminalTarget()), true)).toBe("sidebar");
  });

  it("fires from a text field, which is no reason to stand down", () => {
    expect(panelChordFor(chord({ key: "b", ctrl: true }, fieldTarget()), false)).toBe("sidebar");
  });
});
