import { describe, expect, it } from "vitest";
import {
  buildSkillBlock,
  composeSections,
  DOCTRINE_CEILING_CHARS,
  DOCTRINE_SECTION_CEILING_CHARS,
  parseSkillFile,
  resolveSections,
  SECTION_SEPARATOR,
  TRUNCATION_NOTICE,
  validateSections,
  type SkillSection,
  type SkillSource,
} from "./skillLoader";

// ── Fixtures ───────────────────────────────────────────────────────────

const FIXTURES: Record<string, string> = {
  "typography.md": `---
slug: typography
title: Typography
requires: [color]
---

Use at least 1.5 line-height for body text.

Keep measure between 45 and 75 characters.
`,
  "color.md": `---
slug: color
title: Color
---

Never signal state by colour alone.
Provide sufficient contrast.
`,
  "motion.md": `---
slug: motion
title: Motion
requires: [color]
---

Use consistent easing curves.
`,
  "spacing.md": `---
slug: spacing
title: Spacing
requires: []
---

Use an 8px grid.
`,
  "standalone.md": `---
slug: standalone
title: Standalone
---

This section has no requires.
`,
  "cycles-a.md": `---
slug: cycles-a
title: Cycles A
requires: [cycles-b]
---

A depends on B.
`,
  "cycles-b.md": `---
slug: cycles-b
title: Cycles B
requires: [cycles-a]
---

B depends on A.
`,
  "unknown-dep.md": `---
slug: unknown-dep
title: Unknown Dep
requires: [nonexistent]
---

This references a slug that does not exist.
`,
  "missing-slug.md": `---
title: Missing Slug
---

No slug field here.
`,
  "bad-slug.md": `---
slug: Bad-Slug
title: Bad Slug
---

Uppercase slug.
`,
  "duplicate-key.md": `---
slug: dup
title: Dup
slug: dup-again
---

Two slugs declared.
`,
  "no-fence.md": `No front matter here at all.`,
  "unclosed.md": `---
slug: unclosed
title: Unclosed
requires: []

This fence never closes.
`,
  "mismatch.md": `---
slug: mismatch-slug
title: Mismatch
---

Filename would be mismatch.md, not mismatch-slug.
`,
};

function source(path: string): SkillSource {
  const text = FIXTURES[path];
  if (text === undefined) throw new Error(`Fixture not found: ${path}`);
  return { path, text };
}

function makeSection(
  slug: string,
  title: string,
  body: string,
  requires: readonly string[] = [],
): SkillSection {
  return { slug, title, requires, body };
}

// ── Parse ──────────────────────────────────────────────────────────────

describe("parseSkillFile", () => {
  it("parses a valid skill file", () => {
    const result = parseSkillFile("typography.md", FIXTURES["typography.md"]!);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.section.slug).toBe("typography");
    expect(result.section.title).toBe("Typography");
    expect(result.section.requires).toEqual(["color"]);
    expect(result.section.body).toContain("1.5 line-height");
  });

  it("parses a file with empty requires", () => {
    const result = parseSkillFile("spacing.md", FIXTURES["spacing.md"]!);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.section.requires).toEqual([]);
  });

  it("parses a file with no requires field at all", () => {
    const result = parseSkillFile("standalone.md", FIXTURES["standalone.md"]!);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.section.requires).toEqual([]);
  });

  it("fails on missing opening --- fence", () => {
    const result = parseSkillFile("no-fence.md", FIXTURES["no-fence.md"]!);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.kind).toBe("malformed_front_matter");
  });

  it("fails on missing closing --- fence", () => {
    const result = parseSkillFile("unclosed.md", FIXTURES["unclosed.md"]!);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.kind).toBe("malformed_front_matter");
  });

  it("fails on invalid slug (uppercase)", () => {
    const result = parseSkillFile("bad-slug.md", FIXTURES["bad-slug.md"]!);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.kind).toBe("invalid_slug");
  });

  it("fails on slug/filename mismatch", () => {
    const result = parseSkillFile("mismatch.md", FIXTURES["mismatch.md"]!);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.kind).toBe("slug_filename_mismatch");
    if (result.error.kind === "slug_filename_mismatch") {
      expect(result.error.slug).toBe("mismatch-slug");
      expect(result.error.filename).toBe("mismatch");
    }
  });

  it("fails on duplicate key", () => {
    const result = parseSkillFile("duplicate-key.md", FIXTURES["duplicate-key.md"]!);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.kind).toBe("duplicate_key");
  });

  it("fails on missing slug field", () => {
    const result = parseSkillFile("missing-slug.md", FIXTURES["missing-slug.md"]!);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.kind).toBe("malformed_front_matter");
  });

  it("each parse failure kind is distinguishable", () => {
    const cases = [
      { path: "no-fence.md", kind: "malformed_front_matter" },
      { path: "unclosed.md", kind: "malformed_front_matter" },
      { path: "bad-slug.md", kind: "invalid_slug" },
      { path: "mismatch.md", kind: "slug_filename_mismatch" },
      { path: "duplicate-key.md", kind: "duplicate_key" },
      { path: "missing-slug.md", kind: "malformed_front_matter" },
    ] as const;
    for (const { path: p, kind } of cases) {
      const result = parseSkillFile(p, FIXTURES[p]!);
      expect(result.ok).toBe(false);
      if (!result.ok) {
        expect(result.error.kind).toBe(kind);
      }
    }
  });
});

// ── Dependency resolution ──────────────────────────────────────────────

function sections(...slugs: string[]): SkillSection[] {
  return slugs.map((slug) => {
    const result = parseSkillFile(`${slug}.md`, FIXTURES[`${slug}.md`]!);
    if (!result.ok) throw new Error(`Fixture ${slug} did not parse`);
    return result.section;
  });
}

describe("resolveSections", () => {
  it("pulls transitive dependencies", () => {
    const all = sections("typography", "color", "motion");
    const result = resolveSections(["typography"], all);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const slugs = result.order.map((s) => s.slug);
    // typography requires color, so both must appear
    expect(slugs).toContain("typography");
    expect(slugs).toContain("color");
  });

  it("detects a cycle and reports the path", () => {
    const all = sections("cycles-a", "cycles-b");
    const result = resolveSections(["cycles-a"], all);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.kind).toBe("cycle");
    if (result.error.kind === "cycle") {
      expect(result.error.path).toContain("cycles-a");
      expect(result.error.path).toContain("cycles-b");
    }
  });

  it("fails on unknown requested slug", () => {
    const all = sections("color");
    const result = resolveSections(["nonexistent"], all);
    expect(result.ok).toBe(false);
    if (result.ok) return;
    if (result.error.kind === "unknown_slug") {
      expect(result.error.slug).toBe("nonexistent");
    } else {
      // Force failure — expected unknown_slug but got something else
      expect(result.error.kind).toBe("unknown_slug");
    }
  });

  it("tolerates unknown requires slugs (silently skipped)", () => {
    const all = sections("unknown-dep");
    const result = resolveSections(["unknown-dep"], all);
    // Should succeed — unknown-dep has nonexistent in requires, which is tolerated
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.order.map((s) => s.slug)).toEqual(["unknown-dep"]);
  });
});

// ── Ordering ───────────────────────────────────────────────────────────

describe("ordering rules", () => {
  it("explicitly requested outranks dependencies", () => {
    // typography requires color. Both requested explicitly.
    // Explicit sections come first.
    const all = sections("typography", "color");
    const result = resolveSections(["typography", "color"], all);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const slugs = result.order.map((s) => s.slug);
    // Both are explicit at depth 0, tiebreak is alphabetical: color before typography
    expect(slugs).toEqual(["color", "typography"]);
  });

  it("shallower dependency depth first within a tier", () => {
    // Create a chain: deep-a requires deep-b, deep-b requires deep-c
    const deepC = makeSection("deep-c", "Deep C", "c", []);
    const deepB = makeSection("deep-b", "Deep B", "b", ["deep-c"]);
    const deepA = makeSection("deep-a", "Deep A", "a", ["deep-b"]);
    const all = [deepA, deepB, deepC];

    // Request only deep-a. Closure: deep-a (depth 0, explicit),
    // deep-b (depth 1, dep), deep-c (depth 2, dep).
    const result = resolveSections(["deep-a"], all);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const slugs = result.order.map((s) => s.slug);
    // Explicit tier: deep-a (depth 0)
    // Dependency tier: deep-b (depth 1), deep-c (depth 2)
    expect(slugs).toEqual(["deep-a", "deep-b", "deep-c"]);
  });

  it("ties break alphabetically by slug", () => {
    // Both explicit, same depth → alphabetical
    const all = sections("motion", "color", "spacing", "typography");
    const result = resolveSections(["motion", "color", "spacing", "typography"], all);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    const slugs = result.order.map((s) => s.slug);
    expect(slugs).toEqual(["color", "motion", "spacing", "typography"]);
  });

  it("ordering is stable across input permutations", () => {
    const all = sections("motion", "color", "spacing", "typography");
    const reversed = [...all].reverse();

    const first = resolveSections(["motion", "color", "spacing", "typography"], all);
    const second = resolveSections(["motion", "color", "spacing", "typography"], reversed);

    expect(first.ok).toBe(true);
    expect(second.ok).toBe(true);
    if (!first.ok || !second.ok) return;
    expect(first.order.map((s) => s.slug)).toEqual(second.order.map((s) => s.slug));
  });
});

// ── Composition ────────────────────────────────────────────────────────

describe("composeSections", () => {
  it("leaves an untruncated block unchanged", () => {
    const all = sections("color", "spacing");
    const expected = all
      .map((section) => `## ${section.title}\n\n${section.body}`)
      .join(SECTION_SEPARATOR);
    const result = composeSections(all);

    expect(result.dropped).toEqual([]);
    expect(result.text).toBe(expected);
    expect(result.text).not.toContain(TRUNCATION_NOTICE);
  });

  it("announces omitted sections inside the composed block", () => {
    const result = composeSections(sections("typography", "color"), 50);

    expect(result.dropped.length).toBeGreaterThan(0);
    expect(result.text).toContain(TRUNCATION_NOTICE);
  });

  it("refits to reserve room for its truncation notice", () => {
    const reserved = SECTION_SEPARATOR.length + TRUNCATION_NOTICE.length;
    const firstTitle = "First";
    const secondTitle = "Second";
    const firstHeader = `## ${firstTitle}\n\n`;
    const secondHeader = `## ${secondTitle}\n\n`;
    const firstBlockTarget = Math.floor((DOCTRINE_CEILING_CHARS - reserved) / 2);
    const firstBodyLength = firstBlockTarget - firstHeader.length;
    const first = makeSection("first", firstTitle, "x".repeat(firstBodyLength));
    const firstBlockLength = firstHeader.length + firstBodyLength;
    const fitMargin = "x".length;
    const firstPassCombinedLength = DOCTRINE_CEILING_CHARS - fitMargin;
    const secondBodyLength =
      firstPassCombinedLength - SECTION_SEPARATOR.length - firstBlockLength - secondHeader.length;
    const second = makeSection("second", secondTitle, "x".repeat(secondBodyLength));
    const thirdTitle = "Third";
    const thirdHeader = `## ${thirdTitle}\n\n`;
    const thirdBodyLength =
      DOCTRINE_CEILING_CHARS -
      reserved -
      firstBlockLength -
      SECTION_SEPARATOR.length -
      thirdHeader.length +
      fitMargin;
    const third = makeSection("third", thirdTitle, "x".repeat(thirdBodyLength));

    expect(composeSections([first, second]).dropped).toEqual([]);

    const result = composeSections([first, second, third]);

    expect(result.text).toContain(TRUNCATION_NOTICE);
    expect(result.dropped).toEqual(["second", "third"]);
    // The property the reservation exists for: appending the notice must not push
    // the block back over the ceiling it was just fitted to.
    expect(result.totalChars).toBeLessThanOrEqual(DOCTRINE_CEILING_CHARS);
  });

  it("drops whole sections from the end when over the ceiling", () => {
    const all = sections("typography", "color");
    // Set a very low ceiling so only one fits
    const result = composeSections(all, 50);
    expect(result.dropped.length).toBeGreaterThan(0);
    // The first section is included
    expect(result.text).toContain("Typography");
    // The dropped list names the sections that were cut
    expect(result.dropped).toContain("color");
  });

  it("states which sections were dropped", () => {
    const all = sections("typography", "color", "motion");
    const result = composeSections(all, 100);
    // At least one should be dropped at this low ceiling
    expect(result.dropped.length).toBeGreaterThan(0);
    // dropped slugs must be a subset of the input
    for (const slug of result.dropped) {
      expect(all.some((s) => s.slug === slug)).toBe(true);
    }
  });

  it("includes a single section that alone exceeds the ceiling", () => {
    const huge = makeSection("huge", "Huge", "x".repeat(DOCTRINE_CEILING_CHARS + 100));
    const result = composeSections([huge]);
    expect(result.text).toContain("Huge");
    expect(result.dropped).toEqual([]);
    expect(result.totalChars).toBeGreaterThan(DOCTRINE_CEILING_CHARS);
  });

  it("drops a second section when the first already fills the ceiling", () => {
    // First section body such that the composed block is exactly at ceiling.
    // Header is `## First\n\n` = 9 chars.
    const first = makeSection("first", "First", "x".repeat(DOCTRINE_CEILING_CHARS - 9));
    const second = makeSection("second", "Second", "content");
    const result = composeSections([first, second]);
    expect(result.dropped).toEqual(["second"]);
    expect(result.text).toContain("First");
    expect(result.text).not.toContain("Second");
  });

  it("returns empty block for empty input", () => {
    const result = composeSections([], 100);
    expect(result.text).toBe("");
    expect(result.dropped).toEqual([]);
    expect(result.totalChars).toBe(0);
  });

  it("never cuts a section mid-rule", () => {
    const all = sections("typography", "color");
    const result = composeSections(all, 100);
    // If color is dropped, it must be entirely absent
    if (result.dropped.includes("color")) {
      expect(result.text).not.toContain("Color");
    }
  });
});

// ── Tolerant runtime ───────────────────────────────────────────────────

describe("buildSkillBlock", () => {
  it("returns a usable block for valid sources", () => {
    const sources = [source("typography.md"), source("color.md")];
    const result = buildSkillBlock(sources, ["typography"]);
    expect(result.text).toContain("Typography");
    expect(result.text).toContain("1.5 line-height");
    // color is a transitive dependency of typography
    expect(result.text).toContain("Color");
  });

  it("skips malformed sources and still returns a block", () => {
    const sources = [
      source("typography.md"),
      source("no-fence.md"), // malformed
      source("color.md"),
    ];
    const result = buildSkillBlock(sources, ["typography"]);
    // Should still work despite the malformed file
    expect(result.text).toContain("Typography");
    expect(result.text).toContain("Color");
  });

  it("skips sources with cycles and returns a partial block", () => {
    const sources = [source("color.md"), source("cycles-a.md"), source("cycles-b.md")];
    const result = buildSkillBlock(sources, ["color", "cycles-a"]);
    // cycles-a and cycles-b form a cycle. buildSkillBlock should exclude the
    // cyclic section (cycles-a) and still include the non-cyclic one (color).
    expect(result.text).toContain("Color");
    expect(result.text).not.toContain("Cycles A");
  });

  it("recovers a valid section from two disjoint cycles", () => {
    // Two independent cycles (a↔b, c↔d) plus a valid section e.
    const cycleA = makeSection("cycle-a", "Cycle A", "a", ["cycle-b"]);
    const cycleB = makeSection("cycle-b", "Cycle B", "b", ["cycle-a"]);
    const cycleC = makeSection("cycle-c", "Cycle C", "c", ["cycle-d"]);
    const cycleD = makeSection("cycle-d", "Cycle D", "d", ["cycle-c"]);
    const valid = makeSection("valid-e", "Valid E", "e content here");
    const all = [cycleA, cycleB, cycleC, cycleD, valid];
    const sources: SkillSource[] = all.map((s) => ({
      path: `${s.slug}.md`,
      text: `---\nslug: ${s.slug}\ntitle: ${s.title}\nrequires: [${s.requires.join(", ")}]\n---\n\n${s.body}`,
    }));
    const result = buildSkillBlock(sources, ["valid-e", "cycle-a", "cycle-c"]);
    // valid-e has no cycle and should survive.
    expect(result.text).toContain("Valid E");
    // cyclic sections should not appear.
    expect(result.text).not.toContain("Cycle A");
    expect(result.text).not.toContain("Cycle C");
  });

  it("terminates when cycle filtering makes no progress", () => {
    // A cycle whose path references slugs not in the section list cannot
    // make progress. The loop must not spin.
    const sources: SkillSource[] = [
      {
        path: "valid.md",
        text: `---\nslug: valid\ntitle: Valid\nrequires: []\n---\n\ncontent`,
      },
    ];
    // Request a slug that does not exist, which is an unknown_slug error
    // (not a cycle).  The loop handles this by returning empty.
    const result = buildSkillBlock(sources, ["nonexistent"]);
    expect(result.text).toBe("");
  });

  it("terminates when a cycle path does not intersect sections", () => {
    // If resolveSections returns a cycle whose path slugs are NOT in the
    // section list (e.g., a requires referencing itself in a way that
    // produces an empty path), the loop must not spin.
    // This guards against the edge case where filtering removes nothing.
    const sources: SkillSource[] = [
      {
        path: "self.md",
        text: `---\nslug: self\ntitle: Self\nrequires: [self]\n---\n\ncontent`,
      },
    ];
    const result = buildSkillBlock(sources, ["self"]);
    // A self-cycle is still a cycle; the block should be empty.
    expect(result.text).toBe("");
  });

  it("returns an empty block when every source is invalid", () => {
    const sources = [source("no-fence.md"), source("bad-slug.md")];
    const result = buildSkillBlock(sources, ["typography"]);
    expect(result.text).toBe("");
    expect(result.dropped).toEqual([]);
  });

  it("returns an empty block for empty input", () => {
    const result = buildSkillBlock([], ["typography"]);
    expect(result.text).toBe("");
    expect(result.dropped).toEqual([]);
  });

  it("returns an empty block for empty requested slugs", () => {
    const sources = [source("typography.md")];
    const result = buildSkillBlock(sources, []);
    expect(result.text).toBe("");
  });
});

// ── Strict first-party check ───────────────────────────────────────────

describe("validateSections", () => {
  it("passes valid sources", () => {
    const sources = [source("typography.md"), source("color.md")];
    const errors = validateSections(sources);
    expect(errors).toEqual([]);
  });

  it("fails on malformed front matter", () => {
    const sources = [source("no-fence.md")];
    const errors = validateSections(sources);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.error.kind === "malformed_front_matter")).toBe(true);
  });

  it("fails on slug/filename mismatch", () => {
    const sources = [source("mismatch.md")];
    const errors = validateSections(sources);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.error.kind === "slug_filename_mismatch")).toBe(true);
  });

  it("fails on duplicate slugs across sources", () => {
    const sources = [source("color.md"), source("color.md")];
    const errors = validateSections(sources);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.error.kind === "duplicate_key")).toBe(true);
  });

  it("fails on unknown requires slug", () => {
    const sources = [source("unknown-dep.md")];
    const errors = validateSections(sources);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.error.kind === "unknown_slug")).toBe(true);
  });

  it("fails on a cycle in the dependency graph", () => {
    const sources = [source("cycles-a.md"), source("cycles-b.md")];
    const errors = validateSections(sources);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.error.kind === "cycle")).toBe(true);
  });

  it("returns all errors, not just the first", () => {
    // A source that's both malformed and would cause other issues
    const sources = [source("no-fence.md"), source("bad-slug.md")];
    const errors = validateSections(sources);
    expect(errors.length).toBe(2);
  });

  it("rejects input that buildSkillBlock tolerates", () => {
    // validateSections must fail where buildSkillBlock succeeds
    const tolerant = buildSkillBlock([source("unknown-dep.md")], ["unknown-dep"]);
    expect(tolerant.text).not.toBe("");

    const errors = validateSections([source("unknown-dep.md")]);
    expect(errors.length).toBeGreaterThan(0);
  });

  it("rejects a section whose body exceeds the per-section ceiling", () => {
    const oversized = makeSection(
      "oversized",
      "Oversized",
      "x".repeat(DOCTRINE_SECTION_CEILING_CHARS + 1),
    );
    const sources: SkillSource[] = [
      {
        path: "oversized.md",
        text: `---\nslug: oversized\ntitle: Oversized\nrequires: []\n---\n\n${oversized.body}`,
      },
    ];
    const errors = validateSections(sources);
    expect(errors.length).toBe(1);
    expect(errors[0]!.error.kind).toBe("section_too_large");
    if (errors[0]!.error.kind === "section_too_large") {
      expect(errors[0]!.error.slug).toBe("oversized");
      expect(errors[0]!.error.length).toBe(DOCTRINE_SECTION_CEILING_CHARS + 1);
    }
  });

  it("rejects a section with an empty body", () => {
    const errors = validateSections([
      {
        path: "empty.md",
        text: "---\nslug: empty\ntitle: Empty\nrequires: []\n---\n",
      },
    ]);

    expect(errors).toHaveLength(1);
    expect(errors[0]!.error).toEqual({ kind: "section_empty", slug: "empty" });
  });

  it("rejects a section with a whitespace-only body", () => {
    const errors = validateSections([
      {
        path: "whitespace.md",
        text: "---\nslug: whitespace\ntitle: Whitespace\nrequires: []\n---\n \n \t\n",
      },
    ]);

    expect(errors).toHaveLength(1);
    expect(errors[0]!.error).toEqual({ kind: "section_empty", slug: "whitespace" });
  });

  it("accepts a section whose body exactly fits the ceiling", () => {
    const exact = makeSection("exact", "Exact", "x".repeat(DOCTRINE_SECTION_CEILING_CHARS));
    const sources: SkillSource[] = [
      {
        path: "exact.md",
        text: `---\nslug: exact\ntitle: Exact\nrequires: []\n---\n\n${exact.body}`,
      },
    ];
    const errors = validateSections(sources);
    // body.length === ceiling is OK (only body > ceiling is rejected)
    expect(errors.filter((e) => e.error.kind === "section_too_large")).toEqual([]);
  });
});
