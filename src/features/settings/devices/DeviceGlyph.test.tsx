// @vitest-environment happy-dom

// The paired row's device glyph: one neutral mark for every row, hidden
// from assistive tech (the adjacent name carries the meaning).
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { DeviceGlyph } from "./DeviceGlyph";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("DeviceGlyph", () => {
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
  });

  it("renders one stroke mark and hides it from assistive tech", async () => {
    await act(async () => root.render(<DeviceGlyph />));
    const mark = container.querySelector("svg");
    if (mark === null) throw new Error("device glyph did not render");
    expect(mark.getAttribute("aria-hidden")).toBe("true");
  });

  it("draws its own strokes, never an image or a brand logo", async () => {
    await act(async () => root.render(<DeviceGlyph />));
    expect(container.querySelector("svg path, svg rect, svg circle")).not.toBeNull();
    expect(container.querySelector("img")).toBeNull();
  });
});
