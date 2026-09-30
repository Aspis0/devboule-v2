// @vitest-environment happy-dom

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import {
  fireResize,
  installFrameStub,
  installResizeObserver,
  mountRail,
  scrollTo,
  stubAnchor,
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

/** Three overflowing turns with measured anchors. */
function readyRail(count: number) {
  const items = transcript(count);
  const rail = mountRail(items);
  stubOverflow(rail.conversation, 4000, 500);
  rail.anchors.forEach((anchor, index) =>
    stubAnchor(anchor, rail.conversation, 100 + index * 300, 40),
  );
  rail.rerender([...items]);
  fireResize();
  return rail;
}

function tabIndexes(rail: ReturnType<typeof mountRail>): number[] {
  return rail.dots().map((dot) => dot.tabIndex);
}

function openStop(dot: Element): Element {
  const stop = dot.parentElement;
  if (stop === null) throw new Error("the dot has no stop");
  return stop;
}

describe("the turn rail roving tab stop", () => {
  it("holds one stop on the current turn and moves it as the reader scrolls", () => {
    const rail = readyRail(3);
    expect(rail.dots()).toHaveLength(3);
    expect(tabIndexes(rail)).toEqual([0, -1, -1]);

    scrollTo(rail.conversation, 450);
    expect(tabIndexes(rail)).toEqual([-1, 0, -1]);

    scrollTo(rail.conversation, 900);
    expect(tabIndexes(rail)).toEqual([-1, -1, 0]);
    rail.unmount();
  });

  it("renders no stop with no user turn", () => {
    const items: AgentChatItem[] = [
      { id: "a-1", role: "assistant", text: "no question yet", messageId: null },
    ];
    const rail = mountRail(items);
    stubOverflow(rail.conversation, 2000, 500);
    rail.rerender([...items]);
    fireResize();
    expect(rail.dots()).toHaveLength(0);
    rail.unmount();
  });

  it("keeps one stop on a rendered dot when the first turn loses its measure", () => {
    const rail = readyRail(3);
    // The stop starts on the first dot, before focus ever moves.
    expect(tabIndexes(rail)).toEqual([0, -1, -1]);

    // Its bubble leaves the DOM: the dot unmounts and the stop must leave
    // with it, onto a dot that still renders.
    rail.anchors[0]!.remove();
    rail.rerender(transcript(3));
    fireResize();
    expect(rail.dots()).toHaveLength(2);
    expect(tabIndexes(rail)).toEqual([0, -1]);
    expect(rail.dots()[0]!.getAttribute("aria-label")).toContain("Turn 2 of 3");
    rail.unmount();
  });

  it("keeps the stop on the last-focused dot across blur, so Shift-Tab re-enters there", () => {
    const rail = readyRail(3);
    act(() => rail.dots()[0]!.focus());
    act(() =>
      rail
        .dots()[0]!
        .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })),
    );
    const dots = rail.dots();
    expect(tabIndexes(rail)).toEqual([-1, 0, -1]);

    // Tab out of the rail entirely: the stop stays where focus was.
    act(() => dots[1]!.blur());
    expect(tabIndexes(rail)).toEqual([-1, 0, -1]);

    // Shift-Tab back in lands on the stop, and its preview opens.
    act(() => dots[1]!.focus());
    expect(document.activeElement).toBe(dots[1]);
    expect(openStop(dots[1]!).hasAttribute("data-preview-open")).toBe(true);
    rail.unmount();
  });

  it("moves the stop to the clicked dot after the keyboard moved it", () => {
    const rail = readyRail(3);
    act(() => rail.dots()[0]!.focus());
    act(() =>
      rail
        .dots()[0]!
        .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })),
    );
    act(() =>
      rail
        .dots()[1]!
        .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })),
    );
    expect(tabIndexes(rail)).toEqual([-1, -1, 0]);

    // A click moves DOM focus and the stop together, like the strip.
    act(() => rail.dots()[0]!.click());
    expect(tabIndexes(rail)).toEqual([0, -1, -1]);
    rail.unmount();
  });

  it("moves the stop on pointer focus without opening the preview", () => {
    const rail = readyRail(3);
    act(() => rail.dots()[0]!.focus());
    act(() =>
      rail
        .dots()[0]!
        .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })),
    );
    act(() =>
      rail
        .dots()[1]!
        .dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true })),
    );
    expect(tabIndexes(rail)).toEqual([-1, -1, 0]);

    // A pointer press focuses the first dot without clicking it: the press
    // suppression keeps the preview closed, but the stop still follows focus.
    const first = rail.dots()[0]!;
    act(() => {
      first.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      first.focus();
    });
    expect(openStop(first).hasAttribute("data-preview-open")).toBe(false);
    expect(tabIndexes(rail)).toEqual([0, -1, -1]);
    rail.unmount();
  });

  it("moves the stop off a removed turn instead of losing it", () => {
    const items = transcript(3);
    const rail = mountRail(items);
    stubOverflow(rail.conversation, 4000, 500);
    rail.anchors.forEach((anchor, index) =>
      stubAnchor(anchor, rail.conversation, 100 + index * 300, 40),
    );
    rail.rerender([...items]);
    fireResize();

    // Pin the last dot: the stop sits on it, not on the current first.
    act(() => rail.dots()[2]!.click());
    expect(tabIndexes(rail)).toEqual([-1, -1, 0]);

    // Its turn leaves the transcript; the pin clears and exactly one stop
    // remains, on a dot that still exists.
    rail.rerender(items.slice(0, 4));
    fireResize();
    const dots = rail.dots();
    expect(dots).toHaveLength(2);
    expect(tabIndexes(rail).filter((tab) => tab === 0)).toHaveLength(1);
    expect(dots[tabIndexes(rail).indexOf(0)]!.getAttribute("aria-label")).toContain("Turn");
    rail.unmount();
  });
});

describe("rail arrow and jump keys", () => {
  it("moves focus with ArrowUp/ArrowDown and opens each preview, with no wrap", () => {
    const rail = readyRail(3);
    const top = rail.conversation.scrollTop;
    const first = rail.dots()[0]!;

    act(() => first.focus());
    expect(openStop(first).hasAttribute("data-preview-open")).toBe(true);

    const down = new KeyboardEvent("keydown", {
      key: "ArrowDown",
      bubbles: true,
      cancelable: true,
    });
    act(() => first.dispatchEvent(down));
    expect(down.defaultPrevented).toBe(true);
    const dots = rail.dots();
    expect(document.activeElement).toBe(dots[1]);
    expect(openStop(dots[1]!).hasAttribute("data-preview-open")).toBe(true);
    expect(openStop(first).hasAttribute("data-preview-open")).toBe(false);

    // The arrows never scroll the transcript.
    expect(rail.conversation.scrollTop).toBe(top);

    // The last dot holds: no wrap past either end.
    act(() => dots[1]!.focus());
    const end = new KeyboardEvent("keydown", { key: "End", bubbles: true, cancelable: true });
    act(() => dots[1]!.dispatchEvent(end));
    expect(document.activeElement).toBe(dots[2]);
    const pastEnd = new KeyboardEvent("keydown", {
      key: "ArrowDown",
      bubbles: true,
      cancelable: true,
    });
    act(() => dots[2]!.dispatchEvent(pastEnd));
    expect(pastEnd.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(dots[2]);
    expect(rail.conversation.scrollTop).toBe(top);

    const pastStart = new KeyboardEvent("keydown", {
      key: "ArrowUp",
      bubbles: true,
      cancelable: true,
    });
    act(() => dots[2]!.dispatchEvent(pastStart));
    expect(document.activeElement).toBe(dots[1]);
    rail.unmount();
  });

  it("jumps to the first and last dot with Home and End", () => {
    const rail = readyRail(3);
    const dots = rail.dots();
    act(() => dots[2]!.focus());

    const home = new KeyboardEvent("keydown", { key: "Home", bubbles: true, cancelable: true });
    act(() => dots[2]!.dispatchEvent(home));
    expect(home.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(dots[0]);
    expect(openStop(dots[0]!).hasAttribute("data-preview-open")).toBe(true);
    expect(rail.conversation.scrollTop).toBe(0);
    rail.unmount();
  });

  it("still jumps on Enter and Space, keeping focus on the dot", () => {
    const rail = readyRail(2);
    const jump = vi.spyOn(rail.anchors[1], "scrollIntoView");
    const dot = rail.dots()[1]!;
    dot.focus();

    const enter = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    act(() => dot.dispatchEvent(enter));
    expect(jump).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(dot);

    const space = new KeyboardEvent("keydown", { key: " ", bubbles: true, cancelable: true });
    act(() => dot.dispatchEvent(space));
    expect(jump).toHaveBeenCalledTimes(2);
    expect(document.activeElement).toBe(dot);
    rail.unmount();
  });
});
