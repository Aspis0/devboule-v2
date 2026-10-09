// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { readTabSlots } from "./tabSlots";
import { sessionTabElementId } from "./useTabCloseFlow";

/** A chip laid out at a fixed box, as the browser would place it. */
function chip(id: string, left: number, right: number): HTMLElement {
  const element = document.createElement("div");
  element.id = sessionTabElementId(id);
  element.className = "workspace-session-tab";
  element.getBoundingClientRect = () => ({ left, right, top: 0, bottom: 20 }) as DOMRect;
  return element;
}

describe("readTabSlots", () => {
  it("reads every chip's id and horizontal box, in the order they sit", () => {
    const strip = document.createElement("div");
    strip.append(chip("tool:browser:w:page", 0, 120), chip("session-1", 120, 240));
    expect(readTabSlots(strip)).toEqual([
      { id: "tool:browser:w:page", left: 0, right: 120 },
      { id: "session-1", left: 120, right: 240 },
    ]);
  });

  it("ignores anything on the strip that is not a chip", () => {
    const strip = document.createElement("div");
    strip.append(document.createElement("button"), chip("session-1", 0, 100));
    expect(readTabSlots(strip).map((slot) => slot.id)).toEqual(["session-1"]);
  });
});
