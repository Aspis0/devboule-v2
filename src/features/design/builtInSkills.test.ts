import { describe, expect, it } from "vitest";
import {
  buildSkillBlock,
  DOCTRINE_CEILING_CHARS,
  parseSkillFile,
  validateSections,
} from "./skillLoader";
import { builtInSkillSlugs, builtInSkillSources } from "./builtInSkills";

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

  it("contains exactly the two current built-in skill slugs", () => {
    expect(new Set(builtInSkillSlugs())).toEqual(new Set(["anti-ai-slop", "color"]));
  });

  it("composes both built-in skills without dropping either", () => {
    const result = buildSkillBlock(builtInSkillSources(), ["anti-ai-slop", "color"]);

    expect(result.dropped).toEqual([]);
    expect(result.totalChars).toBeLessThan(DOCTRINE_CEILING_CHARS);
    expect(result.text).toContain(`## ${sectionForSlug("anti-ai-slop").title}`);
    expect(result.text).toContain(`## ${sectionForSlug("color").title}`);
  });

  it("composes only the requested built-in skill", () => {
    const result = buildSkillBlock(builtInSkillSources(), ["color"]);
    const color = sectionForSlug("color");
    const antiAiSlop = sectionForSlug("anti-ai-slop");

    expect(result.text).toContain(`## ${color.title}`);
    expect(result.text).not.toContain(`## ${antiAiSlop.title}`);
  });
});
