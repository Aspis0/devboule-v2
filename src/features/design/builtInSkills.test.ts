import { describe, expect, it } from "vitest";
import {
  buildSkillBlock,
  DOCTRINE_CEILING_CHARS,
  parseSkillFile,
  validateSections,
} from "./skillLoader";
import { builtInSkillIndex, builtInSkillSources } from "./builtInSkills";

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
    expect(new Set(builtInSkillIndex().map((entry) => entry.slug))).toEqual(
      new Set(["anti-ai-slop", "color", "state-coverage"]),
    );
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
            },
          ]
        : [];
    });
    const index = builtInSkillIndex();

    expect(index).toEqual(expected);
    expect(index).toHaveLength(sources.length);
    expect(index.every((entry) => entry.description.length > 0)).toBe(true);
  });

  // Every built-in section has to fit together, because "all" is the default mode
  // and truncation there would drop sections in derived order rather than by
  // relevance.  When this fails, the doctrine has outgrown the ceiling and the
  // answer is a selection mechanism, not a bigger number.
  it("composes every built-in skill without dropping any", () => {
    const slugs = builtInSkillIndex().map((entry) => entry.slug);
    const result = buildSkillBlock(builtInSkillSources(), slugs);

    expect(result.dropped).toEqual([]);
    expect(result.totalChars).toBeLessThan(DOCTRINE_CEILING_CHARS);
    for (const slug of slugs) {
      expect(result.text).toContain(`## ${sectionForSlug(slug).title}`);
    }
  });

  it("composes only the requested built-in skill", () => {
    const result = buildSkillBlock(builtInSkillSources(), ["color"]);
    const color = sectionForSlug("color");
    const antiAiSlop = sectionForSlug("anti-ai-slop");

    expect(result.text).toContain(`## ${color.title}`);
    expect(result.text).not.toContain(`## ${antiAiSlop.title}`);
  });
});
