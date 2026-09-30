// @vitest-environment happy-dom

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import * as turnGrouping from "./turnGrouping";
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

/** The card's two formats, spelled out here instead of imported, so a
 * change to the formatter fails this test rather than travels with it. */
function timeOnly(at: Date): string {
  return new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" }).format(at);
}

function dayAndTime(at: Date): string {
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(at);
}

const NOW = new Date(2026, 8, 30, 12, 0, 0);
const THIS_MORNING = new Date(2026, 8, 30, 9, 5);
const TWO_DAYS_AGO = new Date(2026, 8, 28, 14, 32);

/** One turn per entry, each user item carrying that entry as its send
 * time — including the values a nullable daemon field serialises to. */
function timedTurns(times: readonly (number | null)[]): AgentChatItem[] {
  const items: AgentChatItem[] = [];
  times.forEach((atMs, index) => {
    const number = index + 1;
    items.push({
      id: `u-${number}`,
      role: "user",
      text: `Question ${number}`,
      messageId: null,
      // The cast feeds what the daemon's JSON can put on the field,
      // not what the type promises.
      atMs: atMs as unknown as number,
    });
    items.push({ id: `a-${number}`, role: "assistant", text: `Answer ${number}`, messageId: null });
  });
  return items;
}

/** A shown rail: overflowing scrollport, measured bubbles, a column wide
 * enough for the preview card to fit. */
function readyRail(items: readonly AgentChatItem[]) {
  const rail = mountRail(items);
  stubOverflow(rail.conversation, 60_000, 500);
  stubColumnWidth(rail.content, 680);
  rail.anchors.forEach((anchor, index) => {
    stubAnchor(anchor, rail.conversation, 100 + index * 600, 40);
    stubBubble(anchor, 300);
  });
  rail.rerender([...items]);
  fireResize();
  return rail;
}

beforeEach(() => {
  document.body.innerHTML = "";
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
  installFrameStub();
  installResizeObserver();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("the turn rail's time", () => {
  it("shows a turn from today as a clock time and an older turn with its date", () => {
    const rail = readyRail([
      {
        id: "u-1",
        role: "user",
        text: "Asked this morning",
        messageId: null,
        atMs: THIS_MORNING.getTime(),
      },
      { id: "a-1", role: "assistant", text: "Answered", messageId: null },
      {
        id: "u-2",
        role: "user",
        text: "Asked two days ago",
        messageId: null,
        atMs: TWO_DAYS_AGO.getTime(),
      },
      { id: "a-2", role: "assistant", text: "Answered", messageId: null },
    ]);
    const dots = rail.dots();
    expect(dots).toHaveLength(2);
    expect(dots[0]!.querySelector(".turn-rail-preview-time")?.textContent).toBe(
      timeOnly(THIS_MORNING),
    );
    expect(dots[1]!.querySelector(".turn-rail-preview-time")?.textContent).toBe(
      dayAndTime(TWO_DAYS_AGO),
    );
    expect(dots[0]!.getAttribute("aria-label")).toBe(
      `Turn 1 of 2, ${timeOnly(THIS_MORNING)}: Asked this morning`,
    );
    expect(dots[1]!.getAttribute("aria-label")).toBe(
      `Turn 2 of 2, ${dayAndTime(TWO_DAYS_AGO)}: Asked two days ago`,
    );
    rail.unmount();
  });

  it("shows the title alone when the turn carries no time", () => {
    const rail = readyRail(transcript(1));
    const dot = rail.dots()[0]!;
    expect(dot.querySelector(".turn-rail-preview-time")).toBeNull();
    expect(dot.getAttribute("aria-label")).toBe("Turn 1 of 1: Question 1");
    rail.unmount();
  });

  it("shows no time when the send time is not a usable number", () => {
    const rail = readyRail(timedTurns([null, Number.NaN, Number.POSITIVE_INFINITY]));
    const dots = rail.dots();
    expect(dots).toHaveLength(3);
    dots.forEach((dot, index) => {
      expect(dot.querySelector(".turn-rail-preview-time")).toBeNull();
      expect(dot.getAttribute("aria-label")).toBe(`Turn ${index + 1} of 3: Question ${index + 1}`);
    });
    rail.unmount();
  });

  it("shows a turn from another year with its year", () => {
    const lastYear = new Date(2024, 8, 28, 14, 32);
    const rail = readyRail(timedTurns([lastYear.getTime()]));
    expect(rail.dots()[0]!.querySelector(".turn-rail-preview-time")?.textContent).toBe(
      dayAndTime(lastYear),
    );
    rail.unmount();
  });

  it("settles on one day across every dot when a render crosses midnight", () => {
    vi.setSystemTime(new Date(2026, 8, 30, 23, 59, 59));
    const evening = new Date(2026, 8, 30, 23, 0);
    const morning = new Date(2026, 8, 30, 9, 5);
    const rail = readyRail(timedTurns([evening.getTime(), morning.getTime()]));
    const shownTimes = (): (string | undefined)[] =>
      rail.dots().map((dot) => dot.querySelector(".turn-rail-preview-time")?.textContent);
    expect(shownTimes()).toEqual([timeOnly(evening), timeOnly(morning)]);

    vi.setSystemTime(new Date(2026, 9, 1, 0, 0, 5));
    const label = vi.spyOn(turnGrouping, "userTurnLabel");
    // Focus re-renders the rail; of the two dots, only the focused
    // one's own props change.
    act(() => rail.dots()[0]!.focus());
    expect(label.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(shownTimes()).toEqual([dayAndTime(evening), dayAndTime(morning)]);
    label.mockRestore();
    rail.unmount();
  });
});
