// Tests for the slide-deck output mode: the `slides` craft skill and its
// selection. Prompt-shaping and host-threading tests live in
// agentHost.test.tsx beside the groundedPrompt suite, where the session
// harness already exists.

import { describe, expect, it } from "vitest";
import {
  builtInSkillIndex,
  builtInSkillSources,
  MAX_AUTOMATIC_SKILL_SECTIONS,
} from "./builtInSkills";
import {
  DOCTRINE_DESCRIPTION_CEILING_CHARS,
  DOCTRINE_SECTION_CEILING_CHARS,
  parseSkillFile,
} from "./skillLoader";
import { rankSkillsForQuery } from "./skillRanking";

function slidesSection() {
  const source = builtInSkillSources().find((candidate) => candidate.path.endsWith("/slides.md"));
  if (source === undefined) throw new Error("Built-in skill not found: slides");
  const result = parseSkillFile(source.path, source.text);
  if (!result.ok) throw new Error(`Built-in skill did not parse: ${source.path}`);
  return result.section;
}

describe("slides craft skill", () => {
  it("is registered at the end of the priority order", () => {
    // Last, not first: the priority order is the truncation fallback, and a
    // request-specific section must not displace the core head there.
    // Relevance for slide requests comes from the BM25 rank, pinned below.
    const slugs = builtInSkillIndex().map((entry) => entry.slug);
    expect(slugs).toContain("slides");
    expect(slugs[slugs.length - 1]).toBe("slides");
  });

  it("fits the ceilings the other skills respect", () => {
    const section = slidesSection();
    expect(section.body.length).toBeLessThanOrEqual(DOCTRINE_SECTION_CEILING_CHARS);
    expect(section.description.length).toBeLessThanOrEqual(DOCTRINE_DESCRIPTION_CEILING_CHARS);
  });

  it("ends its description with an Apply-whenever sentence", () => {
    expect(slidesSection().description.trim()).toMatch(
      /(?:^|[.!?]\s+)Apply whenever\b[^.!?]*[.!?]$/,
    );
  });

  it("ranks first for slide-deck requests without falling back", () => {
    const index = builtInSkillIndex();
    for (const query of [
      "turn this into a slide deck",
      "make a slide deck for the project presentation",
    ]) {
      const ranking = rankSkillsForQuery(query, index);
      expect(ranking.fallback).toBe(false);
      expect(ranking.slugs[0]).toBe("slides");
    }
  });

  it("leaves the four-section budget intact", () => {
    // The new title must not extend the longest-title term the shared cap
    // derives from; five sections fit under no ceiling.
    expect(MAX_AUTOMATIC_SKILL_SECTIONS).toBe(4);
  });
});
