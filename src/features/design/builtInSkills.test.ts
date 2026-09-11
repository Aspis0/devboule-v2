import { describe, expect, it } from "vitest";
import {
  buildSkillBlock,
  DOCTRINE_CEILING_CHARS,
  parseSkillFile,
  TRUNCATION_NOTICE,
  validateSections,
} from "./skillLoader";
import {
  builtInSkillIndex,
  builtInSkillIndexForOutputMode,
  builtInSkillSources,
  OUTPUT_MODE_SCOPED_SKILL_SLUGS,
} from "./builtInSkills";

const EXPECTED_PRIORITY_ORDER = [
  "anti-ai-slop",
  "typography",
  "color",
  "accessibility",
  "spacing",
  "state-coverage",
  "layout",
  "microcopy",
  "icons",
  "motion",
  "rtl",
  "form-validation",
  "cognition",
  "reference-research",
  "slides",
] as const;

function slugForPath(path: string): string {
  return path.split("/").pop()?.split(".")[0] ?? "";
}

function sectionForSlug(slug: string) {
  const source = builtInSkillSources().find((candidate) => slugForPath(candidate.path) === slug);
  if (source === undefined) throw new Error(`Built-in skill not found: ${slug}`);
  const result = parseSkillFile(source.path, source.text);
  if (!result.ok) throw new Error(`Built-in skill did not parse: ${source.path}`);
  return result.section;
}

describe("built-in design skills", () => {
  it("pass the strict first-party validation", () => {
    expect(validateSections(builtInSkillSources())).toEqual([]);
  });

  it("contains exactly the current built-in skill slugs", () => {
    expect(builtInSkillIndex().map((entry) => entry.slug)).toEqual([...EXPECTED_PRIORITY_ORDER]);
  });

  it("indexes each parseable craft file with its front-matter metadata", () => {
    const sources = builtInSkillSources();
    const expected = sources.flatMap((source) => {
      const result = parseSkillFile(source.path, source.text);
      return result.ok
        ? [
            {
              slug: result.section.slug,
              title: result.section.title,
              description: result.section.description,
              body: result.section.body,
            },
          ]
        : [];
    });
    const index = builtInSkillIndex();

    expect([...index].sort((left, right) => left.slug.localeCompare(right.slug))).toEqual(
      [...expected].sort((left, right) => left.slug.localeCompare(right.slug)),
    );
    expect(index).toHaveLength(sources.length);
    expect(index.every((entry) => entry.description.length > 0)).toBe(true);
  });

  it("composes the priority head and truncates the tail of the built-in corpus", () => {
    const slugs = builtInSkillIndex().map((entry) => entry.slug);
    const result = buildSkillBlock(builtInSkillSources(), slugs);
    const includedSlugs = slugs.filter((slug) =>
      result.text.includes(`## ${sectionForSlug(slug).title}`),
    );
    expect(slugs).toEqual([...EXPECTED_PRIORITY_ORDER]);
    expect(includedSlugs).toEqual(EXPECTED_PRIORITY_ORDER.slice(0, 4));
    expect(result.dropped).toEqual(EXPECTED_PRIORITY_ORDER.slice(4));
    expect(result.text).toContain(TRUNCATION_NOTICE);
    expect(result.totalChars).toBeLessThanOrEqual(DOCTRINE_CEILING_CHARS);
  });

  it("composes only the requested built-in skill", () => {
    const result = buildSkillBlock(builtInSkillSources(), ["color"]);
    const color = sectionForSlug("color");
    const antiAiSlop = sectionForSlug("anti-ai-slop");

    expect(result.text).toContain(`## ${color.title}`);
    expect(result.text).not.toContain(`## ${antiAiSlop.title}`);
  });
});

describe("output-mode scoped skills", () => {
  const fullSlugs = (): readonly string[] => builtInSkillIndex().map((entry) => entry.slug);
  const pageSlugs = (): readonly string[] =>
    builtInSkillIndexForOutputMode("page").map((entry) => entry.slug);

  it("records the deck section as scoped to the slides mode", () => {
    expect([...OUTPUT_MODE_SCOPED_SKILL_SLUGS.keys()]).toContain("slides");
    expect(OUTPUT_MODE_SCOPED_SKILL_SLUGS.get("slides")).toBe("slides");
  });

  it("keeps the manual catalogue whole while narrowing the routed index", () => {
    // The settings picker names a section explicitly, so it must keep showing
    // every section. Only the corpus a chooser may route over is narrowed.
    expect(fullSlugs()).toEqual([...EXPECTED_PRIORITY_ORDER]);
    expect(fullSlugs()).toHaveLength(15);
    expect(fullSlugs()).toContain("slides");

    const scoped = [...OUTPUT_MODE_SCOPED_SKILL_SLUGS.keys()];
    expect(pageSlugs()).not.toContain("slides");
    expect(pageSlugs()).toEqual(fullSlugs().filter((slug) => !scoped.includes(slug)));
    // Subsequence, not a re-sort: the priority order the truncation fallback
    // depends on survives the narrowing.
    const source = fullSlugs();
    let cursor = 0;
    for (const slug of pageSlugs()) {
      cursor = source.indexOf(slug, cursor) + 1;
      expect(cursor).toBeGreaterThan(0);
    }

    // Slides mode is unrestricted: the deck section competes like any other.
    expect(builtInSkillIndexForOutputMode("slides").map((entry) => entry.slug)).toEqual(
      fullSlugs(),
    );
  });

  it("treats an unscoped section as available in every mode", () => {
    for (const slug of fullSlugs().filter(
      (candidate) => !OUTPUT_MODE_SCOPED_SKILL_SLUGS.has(candidate),
    )) {
      expect(pageSlugs()).toContain(slug);
    }
  });
});
