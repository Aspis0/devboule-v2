// The tab context menu's entries: Paseo's four close entries in Paseo's
// order (workspace-tab-menu.test.ts:47-56 is the order's source), our Delete
// after them in the destructive tone, and the rule for when each one has
// nothing to act on.

import { describe, expect, it } from "vitest";
import { buildSelectionCloseEntry, buildTabCloseEntries } from "./tabCloseMenu";

function labels(entries: ReturnType<typeof buildTabCloseEntries>): string[] {
  return entries.map((entry) => entry.label);
}

function entry(entries: ReturnType<typeof buildTabCloseEntries>, key: string) {
  const found = entries.find((candidate) => candidate.key === key);
  if (found === undefined) throw new Error(`entry did not build: ${key}`);
  return found;
}

describe("buildTabCloseEntries", () => {
  it("lists Paseo's close entries, then Delete, in order on a middle tab", () => {
    const entries = buildTabCloseEntries(1, 3);
    expect(labels(entries)).toEqual([
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
      "Delete",
    ]);
    expect(entry(entries, "delete").destructive).toBe(true);
    expect(entries.every((candidate) => !candidate.disabled)).toBe(true);
  });

  it("disables Close to the left on the first tab", () => {
    const entries = buildTabCloseEntries(0, 3);
    expect(entry(entries, "left").disabled).toBe(true);
    expect(entry(entries, "right").disabled).toBe(false);
    expect(entry(entries, "others").disabled).toBe(false);
    expect(entry(entries, "close").disabled).toBe(false);
    expect(entry(entries, "delete").disabled).toBe(false);
  });

  it("disables Close to the right on the last tab", () => {
    const entries = buildTabCloseEntries(2, 3);
    expect(entry(entries, "left").disabled).toBe(false);
    expect(entry(entries, "right").disabled).toBe(true);
    expect(entry(entries, "others").disabled).toBe(false);
  });

  it("disables every directional entry on the only tab, never Close or Delete", () => {
    const entries = buildTabCloseEntries(0, 1);
    expect(entry(entries, "left").disabled).toBe(true);
    expect(entry(entries, "right").disabled).toBe(true);
    expect(entry(entries, "others").disabled).toBe(true);
    expect(entry(entries, "close").disabled).toBe(false);
    expect(entry(entries, "delete").disabled).toBe(false);
  });
});

describe("buildSelectionCloseEntry", () => {
  it("names the selection's size", () => {
    expect(buildSelectionCloseEntry(3)).toEqual({
      key: "close-selection",
      label: "Close 3 tabs",
      disabled: false,
    });
  });

  it("reads as a single close when one tab is selected", () => {
    expect(buildSelectionCloseEntry(1).label).toBe("Close");
  });

  it("never carries Delete — the selection menu offers no destruction", () => {
    expect(buildSelectionCloseEntry(3).destructive).toBeUndefined();
  });
});
