import { describe, expect, it } from "vitest";
import {
  cloneSectionNotes,
  formatSectionNotesScope,
  resolveSectionNotes,
  sanitizeSectionNotes,
} from "./sectionNotes";

describe("sanitizeSectionNotes", () => {
  it("keeps well-formed notes and drops garbage", () => {
    expect(
      sanitizeSectionNotes([
        { anchor: "content", text: "  Make it louder  " },
        { anchor: "", text: "no anchor" },
        { anchor: "x", text: "   " },
        { anchor: "y" },
        "nope",
      ]),
    ).toEqual([{ anchor: "content", text: "Make it louder" }]);
  });

  it("rejects non-arrays and caps length", () => {
    expect(sanitizeSectionNotes(null)).toEqual([]);
    expect(sanitizeSectionNotes(undefined)).toEqual([]);
    const many = new Array(300).fill({ anchor: "a", text: "t" });
    expect(sanitizeSectionNotes(many)).toHaveLength(200);
  });
});

describe("cloneSectionNotes", () => {
  it("copies entries without sharing identity", () => {
    const notes = [{ anchor: "a", text: "t" }];
    const copy = cloneSectionNotes(notes);
    expect(copy).toEqual(notes);
    expect(copy[0]).not.toBe(notes[0]);
  });
});

describe("resolveSectionNotes", () => {
  const notes = [
    { anchor: "content", text: "Louder" },
    { anchor: "gone", text: "Lost?" },
  ];

  it("splits matched and orphaned against the current page", () => {
    const resolved = resolveSectionNotes(notes, new Set(["content"]), true);
    expect(resolved.matched.map((entry) => entry.note.anchor)).toEqual(["content"]);
    expect(resolved.matched[0]?.index).toBe(0);
    expect(resolved.orphans.map((entry) => entry.note.anchor)).toEqual(["gone"]);
    expect(resolved.orphans[0]?.index).toBe(1);
  });

  it("reports no orphans without an artifact to resolve against", () => {
    const resolved = resolveSectionNotes(notes, new Set(), false);
    expect(resolved.orphans).toEqual([]);
    expect(resolved.matched).toHaveLength(2);
  });
});

describe("formatSectionNotesScope", () => {
  const notes = [
    { anchor: "content", text: "Louder CTA" },
    { anchor: "gone", text: "Keep me" },
  ];
  const anchors = new Set(["content"]);

  it("emits the selected section notes plus flagged orphans", () => {
    const block = formatSectionNotesScope("content", notes, anchors, true);
    expect(block).toContain("Section notes:");
    expect(block).toContain("- [content]: Louder CTA");
    expect(block).toContain("Detached notes");
    expect(block).toContain("(anchor not found in the current page): Keep me");
  });

  it("emits nothing when there is nothing to say", () => {
    expect(formatSectionNotesScope("content", [], anchors, true)).toBe("");
    expect(formatSectionNotesScope(null, [{ anchor: "content", text: "x" }], anchors, true)).toBe(
      "",
    );
  });

  it("never reports orphans without an artifact", () => {
    expect(formatSectionNotesScope(null, notes, new Set(), false)).toBe("");
  });
});
