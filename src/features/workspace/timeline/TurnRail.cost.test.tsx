// @vitest-environment happy-dom

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as turnGrouping from "./turnGrouping";
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

interface ProtoBudget {
  rects: number;
  queries: number;
}

const protoBudget: ProtoBudget = { rects: 0, queries: 0 };
let restorePrototype: (() => void) | null = null;

/** Every getBoundingClientRect and query, counted on the prototype — so
 * the bubbles and the containers are in the total, not only the anchors
 * the element-level instrument below wraps. Stubs that shadow the
 * prototype feed the element-level counters instead; the two never
 * double-count, because an own property hides the prototype one. */
function installPrototypeBudget(): void {
  const proto = Element.prototype as unknown as {
    getBoundingClientRect: () => DOMRect;
    querySelector: (selectors: string) => Element | null;
    querySelectorAll: (selectors: string) => NodeListOf<Element>;
  };
  const rect = proto.getBoundingClientRect;
  const query = proto.querySelector;
  const queryAll = proto.querySelectorAll;
  proto.getBoundingClientRect = function (this: Element): DOMRect {
    protoBudget.rects += 1;
    return rect.call(this);
  };
  proto.querySelector = function (this: Element, selectors: string): Element | null {
    protoBudget.queries += 1;
    return query.call(this, selectors);
  };
  proto.querySelectorAll = function (this: Element, selectors: string): NodeListOf<Element> {
    protoBudget.queries += 1;
    return queryAll.call(this, selectors);
  };
  restorePrototype = () => {
    proto.getBoundingClientRect = rect;
    proto.querySelector = query;
    proto.querySelectorAll = queryAll;
    restorePrototype = null;
  };
}

function resetProtoBudget(): void {
  // A fresh wrap per measurement window: the element-level instrument
  // runs first and captures the unwrapped originals, so the elements it
  // wraps are counted there and never again through the prototype.
  restorePrototype?.();
  protoBudget.rects = 0;
  protoBudget.queries = 0;
  installPrototypeBudget();
}

beforeEach(() => {
  document.body.innerHTML = "";
  installFrameStub();
  installResizeObserver();
  restorePrototype?.();
  protoBudget.rects = 0;
  protoBudget.queries = 0;
});

afterEach(() => {
  restorePrototype?.();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

interface ReadCounts {
  tops: number;
  rects: number;
  sizes: number;
}

/** Counts reads of one element property: the stubbed accessors and the
 * prototype methods alike, so a forced-layout read cannot hide. */
function countReads(target: object, key: string, onRead: () => void): void {
  let current: object | null = target;
  let descriptor: PropertyDescriptor | undefined;
  while (current !== null && descriptor === undefined) {
    descriptor = Object.getOwnPropertyDescriptor(current, key);
    current = Object.getPrototypeOf(current);
  }
  const read = descriptor?.get === undefined ? undefined : descriptor.get.bind(target);
  const write = descriptor?.set === undefined ? undefined : descriptor.set.bind(target);
  const value = descriptor !== undefined && "value" in descriptor ? descriptor.value : undefined;
  Object.defineProperty(target, key, {
    configurable: true,
    get() {
      onRead();
      return read === undefined ? value : read();
    },
    set(next) {
      if (write !== undefined) write(next);
    },
  });
}

function instrument(rail: ReturnType<typeof mountRail>): ReadCounts {
  const counts: ReadCounts = { tops: 0, rects: 0, sizes: 0 };
  for (const anchor of rail.anchors) {
    countReads(anchor, "offsetTop", () => (counts.tops += 1));
    countReads(anchor, "offsetHeight", () => (counts.tops += 1));
    countReads(anchor, "getBoundingClientRect", () => (counts.rects += 1));
  }
  for (const element of [rail.conversation, rail.content]) {
    // scrollTop is left out: the harness's rect stubs read it on every
    // probe, so counting it would measure the stub, not the rail.
    for (const key of ["scrollHeight", "clientHeight", "clientWidth"]) {
      countReads(element, key, () => (counts.sizes += 1));
    }
    countReads(element, "getBoundingClientRect", () => (counts.rects += 1));
  }
  return counts;
}

function showRail(turnCount: number) {
  const items = transcript(turnCount);
  const rail = mountRail(items);
  stubOverflow(rail.conversation, 60_000, 500);
  rail.anchors.forEach((anchor, index) =>
    stubAnchor(anchor, rail.conversation, 100 + index * 200, 40),
  );
  rail.rerender([...items]);
  fireResize();
  expect(rail.dots()).toHaveLength(turnCount);
  return { rail, items };
}

function layoutTotal(reads: ReadCounts): number {
  return reads.tops + reads.rects + reads.sizes + protoBudget.rects + protoBudget.queries;
}

describe("turn rail read budget at 300 turns", () => {
  it("measures the dots once per geometry change, not twice", () => {
    const { rail, items } = showRail(300);

    // Every bubble moves down 100 px (content above rewrapped): one
    // geometry change, so one measuring pass over the 300 bubbles —
    // offsetTop + offsetHeight each — plus the observer's one probe.
    rail.anchors.forEach((anchor, index) =>
      stubAnchor(anchor, rail.conversation, 200 + index * 200, 40),
    );
    const reads = instrument(rail);
    resetProtoBudget();
    rail.rerender([...items]);
    fireResize();

    console.log(
      `geometry change at 300 turns: ${reads.tops} bubble-top reads, ${reads.rects} rect reads, ` +
        `${reads.sizes} box reads, ${protoBudget.rects} prototype rects, ${protoBudget.queries} queries`,
    );
    expect(reads.tops).toBeGreaterThanOrEqual(600);
    expect(reads.tops).toBeLessThanOrEqual(602);
    expect(reads.sizes).toBeLessThanOrEqual(6);
    // The turn elements are cached per turn list: this pass queries
    // nothing, and the only prototype rects are the 300 bubble widths.
    expect(protoBudget.queries).toBe(0);
    expect(protoBudget.rects).toBe(300);
    expect(reads.rects).toBeLessThanOrEqual(10);
    rail.unmount();
  });

  it("spends no layout reads on one streamed token, observer delivery included", () => {
    const { rail, items } = showRail(300);

    const reads = instrument(rail);
    resetProtoBudget();
    const streamed = items.map((item, index) =>
      index === items.length - 1 && "text" in item ? { ...item, text: `${item.text} chunk` } : item,
    );
    rail.rerender(streamed);
    fireResize();

    console.log(
      `streamed token at 300 turns: ${layoutTotal(reads)} layout reads ` +
        `(${reads.tops} bubble tops, ${reads.rects} rects, ${reads.sizes} box sizes, ` +
        `${protoBudget.rects} prototype rects, ${protoBudget.queries} queries)`,
    );
    // The rerender itself: nothing. The browser's observer delivery for
    // the same token: the probe — the last bubble's two box reads and
    // three box sizes — and nothing else.
    expect(reads.tops).toBeLessThanOrEqual(2);
    expect(reads.rects).toBe(0);
    expect(reads.sizes).toBeLessThanOrEqual(3);
    expect(protoBudget.rects).toBe(0);
    expect(protoBudget.queries).toBe(0);
    rail.unmount();
  });

  it("renders nothing on one streamed token", () => {
    const { rail, items } = showRail(300);

    // A render with a new items array misses the memo and groups the
    // turns; a skipped render never calls it.
    const groupTurns = vi.spyOn(turnGrouping, "userTurns");
    const streamed = items.map((item, index) =>
      index === items.length - 1 && "text" in item ? { ...item, text: `${item.text} chunk` } : item,
    );
    rail.rerender(streamed);
    console.log(`streamed token at 300 turns: ${groupTurns.mock.calls.length} renders`);
    expect(groupTurns).not.toHaveBeenCalled();
    groupTurns.mockRestore();
    rail.unmount();
  });

  it("pays for a new turn arriving: two subtree scans, one measure, every stop", () => {
    const { rail, items } = showRail(300);
    const reads = instrument(rail);
    resetProtoBudget();
    const label = vi.spyOn(turnGrouping, "userTurnLabel");

    const withNewTurn = [
      ...items,
      { id: "u-301", role: "user" as const, text: "Question 301", messageId: null },
      { id: "a-301", role: "assistant" as const, text: "Answer 301", messageId: null },
    ];
    rail.rerender(withNewTurn);

    const renders = label.mock.calls.length;
    console.log(
      `new turn at 300 turns: ${protoBudget.queries} queries, ${reads.tops} bubble-top reads, ` +
        `${protoBudget.rects} prototype rects, ${renders} stop renders`,
    );
    // The turn list changed, so the cache rebuilds — two subtree scans,
    // never a query per turn — and one pass measures all 301 bubbles.
    expect(protoBudget.queries).toBe(2);
    // 300 instrumented anchors × offsetTop + offsetHeight; the 301st row
    // is appended after instrumentation, so its two reads sit outside the
    // element counter (its bubble rect is on the prototype counter).
    expect(reads.tops).toBeGreaterThanOrEqual(600);
    expect(reads.tops).toBeLessThanOrEqual(602);
    expect(protoBudget.rects).toBe(301);
    expect(reads.sizes).toBeLessThanOrEqual(3);
    expect(reads.rects).toBeLessThanOrEqual(10);
    // `count` changed under every stop, so the memo re-renders all 301 —
    // once, not once per measure.
    expect(renders).toBe(301);
    label.mockRestore();
    rail.unmount();
  });

  it("re-renders only the touched dots when a preview opens", () => {
    const { rail } = showRail(300);

    // The dot's label is built inside its own render, so its calls count
    // exactly the stops React actually rendered.
    const label = vi.spyOn(turnGrouping, "userTurnLabel");
    const dots = rail.dots();
    act(() => dots[1].focus());
    const renders = label.mock.calls.length;
    console.log(`preview opened on one of 300 dots: ${renders} stop renders`);
    expect(renders).toBeGreaterThanOrEqual(1);
    expect(renders).toBeLessThanOrEqual(2);
    label.mockRestore();
    rail.unmount();
  });

  it("spends one scroll frame's read budget", () => {
    const { rail } = showRail(300);
    const reads = instrument(rail);
    resetProtoBudget();

    // One frame that leaves the current turn where it was: the container
    // rect, the binary search's probes, and nothing else.
    scrollTo(rail.conversation, 150);
    console.log(
      `scroll frame at 300 turns: ${reads.rects} rect reads, ${reads.sizes} box reads, ` +
        `${reads.tops} bubble-top reads, ${protoBudget.rects} prototype rects, ` +
        `${protoBudget.queries} queries`,
    );
    expect(reads.rects).toBeLessThanOrEqual(10);
    expect(reads.tops).toBe(0);
    expect(reads.sizes).toBeLessThanOrEqual(1);
    expect(protoBudget.rects).toBe(0);
    expect(protoBudget.queries).toBe(0);
    rail.unmount();
  });
});
