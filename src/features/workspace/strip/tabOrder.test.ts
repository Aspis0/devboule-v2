import { describe, expect, it } from "vitest";
import { insertionEdge, insertionIndex, moveTabId, orderStripTabs, type TabSlot } from "./tabOrder";

describe("orderStripTabs", () => {
  it("lays tabs out in the remembered order, whatever their kind", () => {
    const tabs = [{ id: "s1" }, { id: "s2" }, { id: "tool:browser:w:b1" }];
    expect(orderStripTabs(tabs, ["tool:browser:w:b1", "s1", "s2"]).map((tab) => tab.id)).toEqual([
      "tool:browser:w:b1",
      "s1",
      "s2",
    ]);
  });

  it("appends a tab the order does not name, in the order it was composed", () => {
    const tabs = [{ id: "s1" }, { id: "s2" }, { id: "s3" }];
    expect(orderStripTabs(tabs, ["s3"]).map((tab) => tab.id)).toEqual(["s3", "s1", "s2"]);
  });

  it("ignores ids in the order that no open tab carries", () => {
    const tabs = [{ id: "s1" }, { id: "s2" }];
    expect(orderStripTabs(tabs, ["gone", "s2", "s1"]).map((tab) => tab.id)).toEqual(["s2", "s1"]);
  });

  it("returns the composed array itself when there is no order to apply", () => {
    const tabs = [{ id: "s1" }, { id: "s2" }];
    expect(orderStripTabs(tabs, [])).toBe(tabs);
  });
});

describe("moveTabId", () => {
  it("moves a tab forward to the index it is given", () => {
    expect(moveTabId(["a", "b", "c"], "a", 2)).toEqual(["b", "c", "a"]);
  });

  it("moves a tab back to the front", () => {
    expect(moveTabId(["a", "b", "c"], "c", 0)).toEqual(["c", "a", "b"]);
  });

  it("clamps an index past the end to the end", () => {
    expect(moveTabId(["a", "b", "c"], "a", 9)).toEqual(["b", "c", "a"]);
  });

  it("adds a tab the order did not hold, at the index given", () => {
    expect(moveTabId(["a", "b"], "x", 1)).toEqual(["a", "x", "b"]);
  });
});

describe("insertionIndex", () => {
  const slots: TabSlot[] = [
    { id: "a", left: 0, right: 100 },
    { id: "b", left: 100, right: 200 },
    { id: "c", left: 200, right: 300 },
  ];

  it("places the drop before every chip whose middle lies to its right", () => {
    expect(insertionIndex(slots, 10, "x")).toBe(0);
    expect(insertionIndex(slots, 150, "x")).toBe(1);
    expect(insertionIndex(slots, 250, "x")).toBe(2);
    expect(insertionIndex(slots, 350, "x")).toBe(3);
  });

  it("does not count the dragged chip against its own position", () => {
    expect(insertionIndex(slots, 150, "b")).toBe(1);
    expect(insertionIndex(slots, 260, "b")).toBe(2);
  });

  it("is zero when the strip has no chips to measure", () => {
    expect(insertionIndex([], 150, "x")).toBe(0);
  });
});

describe("insertionEdge", () => {
  const slots: TabSlot[] = [
    { id: "a", left: 0, right: 100 },
    { id: "b", left: 100, right: 200 },
    { id: "c", left: 200, right: 300 },
  ];

  it("sits on the left edge of the first chip for index zero", () => {
    expect(insertionEdge(slots, "x", 0)).toBe(0);
  });

  it("sits on the right edge of the chip before the gap", () => {
    expect(insertionEdge(slots, "x", 2)).toBe(200);
  });

  it("skips the dragged chip when it measures the gap", () => {
    expect(insertionEdge(slots, "b", 1)).toBe(100);
  });

  it("sits past the last chip at the end of the row", () => {
    expect(insertionEdge(slots, "x", 9)).toBe(300);
  });

  it("is null when there is no other chip to sit beside", () => {
    expect(insertionEdge([], "x", 0)).toBeNull();
    expect(insertionEdge([{ id: "b", left: 0, right: 100 }], "b", 0)).toBeNull();
  });
});
