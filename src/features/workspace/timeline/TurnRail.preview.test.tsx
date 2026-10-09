// @vitest-environment happy-dom

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  fireResize,
  installFrameStub,
  installResizeObserver,
  mountRail,
  stubAnchor,
  stubBubble,
  stubColumnWidth,
  stubOverflow,
  transcript,
} from "./turnRailHarness";

beforeEach(() => {
  document.body.innerHTML = "";
  installFrameStub();
  installResizeObserver();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

/** A shown rail whose column (the content box behind the 32 px gutter)
 * and bubbles carry the given widths. */
function readyPreview(turnBubbleWidths: readonly number[], columnWidth: number) {
  const items = transcript(turnBubbleWidths.length);
  const rail = mountRail(items);
  stubOverflow(rail.conversation, 8000, 500);
  stubColumnWidth(rail.content, columnWidth);
  rail.anchors.forEach((anchor, index) => {
    stubAnchor(anchor, rail.conversation, 100 + index * 600, 40);
    stubBubble(anchor, turnBubbleWidths[index]!);
  });
  rail.rerender([...items]);
  fireResize();
  return rail;
}

function stops(rail: ReturnType<typeof mountRail>): Element[] {
  return [...rail.content.querySelectorAll(".turn-rail-stop")];
}

function openStop(dot: Element): Element {
  const stop = dot.parentElement;
  if (stop === null) throw new Error("the dot has no stop");
  return stop;
}

describe("the turn rail preview card", () => {
  describe("placement", () => {
    it("floats the card over the transcript whatever the bubble's width", () => {
      // The card needs no canvas beside the bubble: it floats above the
      // transcript, so no stop is gated and no dot falls back to a title.
      const rail = readyPreview([476, 300], 640);
      for (const stop of stops(rail)) {
        expect(stop.hasAttribute("data-preview-fits")).toBe(false);
      }
      for (const dot of rail.content.querySelectorAll(".turn-rail-dot")) {
        expect(dot.hasAttribute("title")).toBe(false);
      }
      rail.unmount();
    });
  });

  describe("focus and pinning", () => {
    it("opens the preview on keyboard focus and closes it on blur", () => {
      const rail = readyPreview([300], 680);
      const dot = rail.dots()[0]!;

      act(() => dot.focus());
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(true);

      act(() => dot.blur());
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(false);
      rail.unmount();
    });

    it("lets neither a click nor a jump pin the preview open", () => {
      const rail = readyPreview([300, 300], 680);
      const [first, second] = rail.dots();

      // Keyboard focus opens the preview; the jump closes it again while
      // focus itself stays on the dot.
      act(() => first!.focus());
      expect(openStop(first!).hasAttribute("data-preview-open")).toBe(true);
      const jump = vi.spyOn(rail.anchors[0], "scrollIntoView");
      act(() => {
        first!.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
        );
      });
      expect(jump).toHaveBeenCalledTimes(1);
      expect(openStop(first!).hasAttribute("data-preview-open")).toBe(false);
      expect(document.activeElement).toBe(first);

      // A pointer press focuses the dot too, but the focus-open is
      // suppressed for it: nothing survives the mouse leaving.
      act(() => {
        second!.dispatchEvent(new Event("pointerdown", { bubbles: true }));
        second!.focus();
      });
      expect(openStop(second!).hasAttribute("data-preview-open")).toBe(false);
      rail.unmount();
    });

    it("closes the preview on Escape", () => {
      const rail = readyPreview([300], 680);
      const dot = rail.dots()[0]!;
      act(() => dot.focus());
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(true);

      act(() => {
        dot.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
        );
      });
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(false);
      // Focus itself stays where it was.
      expect(document.activeElement).toBe(dot);
      rail.unmount();
    });

    it("closes the preview when the transcript scrolls", () => {
      const rail = readyPreview([300], 680);
      const dot = rail.dots()[0]!;
      act(() => dot.focus());
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(true);

      act(() => {
        rail.conversation.dispatchEvent(new Event("scroll"));
      });
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(false);
      rail.unmount();
    });

    it("drops a preview whose dot unmounts with the rail", () => {
      const rail = readyPreview([300], 680);
      const dot = rail.dots()[0]!;
      act(() => dot.focus());
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(true);

      // The transcript fits: the rail unmounts, and with it the focused
      // dot — no blur event fires on that path.
      stubOverflow(rail.conversation, 400, 400);
      fireResize();
      expect(rail.content.querySelector("nav.turn-rail")).toBeNull();

      stubOverflow(rail.conversation, 8000, 500);
      fireResize();
      expect(rail.dots()[0].parentElement!.hasAttribute("data-preview-open")).toBe(false);
      rail.unmount();
    });

    it("does not let a pointer press swallow the next keyboard focus", () => {
      const rail = readyPreview([300], 680);
      const dot = rail.dots()[0]!;
      // A press that ends without a click on the button: the release
      // lands elsewhere, so only the press's own events reached the dot.
      act(() => {
        dot.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      });
      act(() => {
        rail.conversation.dispatchEvent(new Event("pointerup", { bubbles: true }));
      });

      act(() => dot.focus());
      expect(openStop(dot).hasAttribute("data-preview-open")).toBe(true);
      rail.unmount();
    });
  });
});
