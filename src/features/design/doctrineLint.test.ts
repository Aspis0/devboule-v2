// Deliberately narrow: this lint does not decide whether a sentence describes craft or
// issues an instruction, because that needs a heuristic that would misfire in both
// directions. Each check below was chosen because a real defect of that exact shape
// shipped and was caught by hand.

import { describe, expect, it } from "vitest";
import { builtInSkillSources } from "./builtInSkills";
import {
  DOCTRINE_DESCRIPTION_CEILING_CHARS,
  DOCTRINE_SECTION_CEILING_CHARS,
  parseSkillFile,
} from "./skillLoader";

function parsedBuiltIns() {
  return builtInSkillSources().map((source) => {
    const result = parseSkillFile(source.path, source.text);
    if (!result.ok) throw new Error(`Built-in doctrine did not parse: ${source.path}`);
    return { path: source.path, section: result.section };
  });
}

function wcagCitationFragments(body: string): readonly string[] {
  const fragments: string[] = [];
  for (const match of body.matchAll(/\bSC\b/gu)) {
    const index = match.index ?? 0;
    const openingBracket = body.lastIndexOf("[", index);
    const closingBracket = body.lastIndexOf("]", index);
    if (openingBracket > closingBracket) continue;

    const fragment = body.slice(index).match(/^SC(?:\s+\S+){0,2}/u)?.[0] ?? "SC";
    fragments.push(fragment.replace(/[,.;:!?]+$/u, ""));
  }
  return fragments;
}

const WCAG_CITATION = /^SC\s+\d+\.\d+\.\d+\s{1,20}\((?:A|AA|AAA)\)$/u;
const APPLY_WHENEVER_SENTENCE = /(?:^|[.!?]\s+)Apply whenever\b[^.!?]*[.!?]$/u;
const FORBIDDEN_BODY_MARKER = /\b(?:TODO|FIXME)\b|\[…\]/gu;

describe("design doctrine lint", () => {
  it("requires every WCAG success criterion citation to include its level", () => {
    const failures: string[] = [];
    for (const { path, section } of parsedBuiltIns()) {
      for (const citation of wcagCitationFragments(section.body)) {
        if (!WCAG_CITATION.test(citation)) {
          failures.push(`${path}: ${JSON.stringify(citation)}`);
        }
      }
    }

    expect(failures, failures.join("\n")).toEqual([]);
  });

  it('requires every description to end with an "Apply whenever" sentence', () => {
    const failures = parsedBuiltIns()
      .filter(({ section }) => !APPLY_WHENEVER_SENTENCE.test(section.description.trim()))
      .map(({ path, section }) => `${path}: ${JSON.stringify(section.description)}`);

    expect(failures, failures.join("\n")).toEqual([]);
  });

  it("keeps bodies and descriptions within their imported ceilings", () => {
    const failures: string[] = [];
    for (const { path, section } of parsedBuiltIns()) {
      if (section.body.length > DOCTRINE_SECTION_CEILING_CHARS) {
        failures.push(
          `${path}: body length ${section.body.length} > ${DOCTRINE_SECTION_CEILING_CHARS}`,
        );
      }
      if (section.description.length > DOCTRINE_DESCRIPTION_CEILING_CHARS) {
        failures.push(
          `${path}: description length ${section.description.length} > ${DOCTRINE_DESCRIPTION_CEILING_CHARS}`,
        );
      }
    }

    expect(failures, failures.join("\n")).toEqual([]);
  });

  it("rejects unresolved body markers", () => {
    const failures: string[] = [];
    for (const { path, section } of parsedBuiltIns()) {
      for (const match of section.body.matchAll(FORBIDDEN_BODY_MARKER)) {
        failures.push(`${path}: ${JSON.stringify(match[0])}`);
      }
    }

    expect(failures, failures.join("\n")).toEqual([]);
  });
});
