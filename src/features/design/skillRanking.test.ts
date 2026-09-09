// Tests for the deterministic lexical skill selector.
//
// Expectations are pinned against the REAL craft corpus (the .md files in
// ./craft), not synthetic fixtures: the point of the ranker is to order
// those 13 sections, so the tests load them and assert the orderings the
// corpus actually produces. Where an intuitively "obvious" expectation does
// not hold against the real text, the test records what the corpus says
// instead of what intuition said — see "make the empty panel say
// something", where state-coverage genuinely outranks microcopy.

import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  rankSkillsForQuery,
  tokenize,
  FALLBACK_SCORE_THRESHOLD,
  type RankableSkill,
} from "./skillRanking";

// ── Real-corpus fixture ────────────────────────────────────────────────

function loadCraftCorpus(): RankableSkill[] {
  const dir = fileURLToPath(new URL("./craft", import.meta.url));
  const skills: RankableSkill[] = [];
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".md")).sort()) {
    const text = readFileSync(`${dir}/${file}`, "utf8");
    const frontMatter = /^---\n([\s\S]*?)\n---\n?/.exec(text);
    if (!frontMatter) throw new Error(`no front matter in craft/${file}`);
    const field = (key: string): string => {
      const m = new RegExp(`^${key}: (.*)$`, "m").exec(frontMatter[1]);
      if (!m) throw new Error(`no ${key} in craft/${file}`);
      return m[1];
    };
    skills.push({
      slug: field("slug"),
      title: field("title"),
      description: field("description"),
      body: text.slice(frontMatter[0].length),
    });
  }
  return skills;
}

const ALL_SLUGS = [
  "accessibility",
  "anti-ai-slop",
  "cognition",
  "color",
  "form-validation",
  "icons",
  "layout",
  "microcopy",
  "motion",
  "rtl",
  "spacing",
  "state-coverage",
  "typography",
] as const;

// This mirrors the BUILT_IN_SKILL_PRIORITY order in builtInSkills.ts (the
// truncation priority): in the non-AI path the caller passes the skills in
// this order, and the ranker must use it as the tie-break and fallback
// order. Reconstructing it here keeps the input-order contract under test
// without importing builtInSkills.ts (which would couple this pure module
// to vite's import.meta.glob).
const PRIORITY_ORDER = [
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
] as const;

function corpusInPriorityOrder(): RankableSkill[] {
  const corpus = loadCraftCorpus();
  const bySlug = new Map(corpus.map((skill) => [skill.slug, skill]));
  return PRIORITY_ORDER.map((slug) => {
    const skill = bySlug.get(slug);
    if (!skill) throw new Error(`craft corpus missing slug: ${slug}`);
    return skill;
  });
}

// ── Rankings on the real corpus ────────────────────────────────────────

describe("rankSkillsForQuery on the real craft corpus", () => {
  it("ranks microcopy and state-coverage at the top for an empty-state copy query", () => {
    // Both sections are about empty states, and the corpus genuinely puts
    // state-coverage first: "panel" and "empty" hit its four-state body
    // repeatedly, while microcopy matches through its "empty states" and
    // "empty result" description clauses plus "empty panels". Microcopy
    // ranks a clear second (score 2.616 against 1.436 for third place), so
    // a top-2 selection includes it; the test pins the corpus's verdict
    // rather than the intuition that microcopy must win.
    const ranking = rankSkillsForQuery("make the empty panel say something", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("state-coverage");
    expect(ranking.slugs[1]).toBe("microcopy");
    expect(ranking.slugs[2]).toBe("spacing");
  });

  it("ranks color then accessibility for a contrast complaint", () => {
    // "contrast" is a gate in the color description and a WCAG criterion in
    // the accessibility description; "button" also feeds microcopy.
    const ranking = rankSkillsForQuery("the button contrast is too low", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("color");
    expect(ranking.slugs[1]).toBe("accessibility");
  });

  it("ranks motion first for an animation request", () => {
    // "animate" and "opening" only exist in motion; "drawer" matches
    // nothing. The single matching token still lifts motion decisively.
    const ranking = rankSkillsForQuery("animate the drawer opening", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("motion");
  });

  it("ranks rtl first for an Arabic/Hebrew request", () => {
    const ranking = rankSkillsForQuery("arabic and hebrew users", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("rtl");
  });

  it("ranks form-validation first for premature form errors", () => {
    const ranking = rankSkillsForQuery("form shows an error before I finish typing", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("form-validation");
  });

  it("ranks accessibility first for a small hit-target complaint", () => {
    const ranking = rankSkillsForQuery("hit target too small on mobile", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("accessibility");
    // icons owns hit-area too; cognition's Fitts paragraph trails.
    expect(ranking.slugs.slice(1, 3)).toEqual(["cognition", "icons"]);
  });

  it("ranks state-coverage first for a loading-skeleton request", () => {
    const ranking = rankSkillsForQuery("loading skeleton for the list", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("state-coverage");
  });

  it("ranks anti-ai-slop first for dark mode and card-slop complaints", () => {
    const dark = rankSkillsForQuery("dark mode by default", corpusInPriorityOrder());
    expect(dark.fallback).toBe(false);
    expect(dark.slugs[0]).toBe("anti-ai-slop");
    expect(dark.slugs[1]).toBe("color");
  });

  it("ranks typography first for a line-length complaint", () => {
    const ranking = rankSkillsForQuery("line length too wide", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("typography");
  });

  it("ranks layout first for a grid-alignment request", () => {
    const ranking = rankSkillsForQuery("align everything to a grid", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("layout");
  });

  it("ranks spacing first for a padding request", () => {
    const ranking = rankSkillsForQuery("more padding between the sections", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("spacing");
  });

  it("ranks icons first for a too-small icon", () => {
    const ranking = rankSkillsForQuery("the icon is too small", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("icons");
    expect(ranking.slugs[1]).toBe("accessibility");
  });

  it("returns the COMPLETE ranking, never a truncated head", () => {
    // Truncation to ~4 sections is the caller's job.
    const ranking = rankSkillsForQuery("more padding between the sections", corpusInPriorityOrder());
    expect(ranking.slugs).toHaveLength(ALL_SLUGS.length);
    expect([...ranking.slugs].sort()).toEqual([...ALL_SLUGS].sort());
  });

  it("is deterministic: same query, same ranking, twice", () => {
    const corpus = corpusInPriorityOrder();
    const first = rankSkillsForQuery("the button contrast is too low", corpus);
    const second = rankSkillsForQuery("the button contrast is too low", corpus);
    expect(second).toEqual(first);
  });
});

// ── Fallback behaviour ─────────────────────────────────────────────────

describe("rankSkillsForQuery fallback", () => {
  it("falls back on an empty or stopword-only query, keeping input order", () => {
    const corpus = corpusInPriorityOrder();
    for (const query of ["", "   ", "the of and to", "???"]) {
      const ranking = rankSkillsForQuery(query, corpus);
      expect(ranking.fallback).toBe(true);
      expect(ranking.slugs).toEqual(PRIORITY_ORDER);
    }
  });

  it("falls back on a vague query that scores below the threshold", () => {
    // "make it prettier": "prettier" matches nothing, "make" matches only
    // scattered body occurrences; the measured best score is ~0.28, well
    // under FALLBACK_SCORE_THRESHOLD.
    const ranking = rankSkillsForQuery("make it prettier", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(true);
    expect(ranking.slugs).toEqual(PRIORITY_ORDER);
  });

  it("falls back when no query token exists in any section", () => {
    const ranking = rankSkillsForQuery("quantum flux capacitor", corpusInPriorityOrder());
    expect(ranking.fallback).toBe(true);
    expect(ranking.slugs).toEqual(PRIORITY_ORDER);
  });

  it("falls back on an empty index", () => {
    const ranking = rankSkillsForQuery("color contrast", []);
    expect(ranking.fallback).toBe(true);
    expect(ranking.slugs).toEqual([]);
  });
});

// ── Determinism and tie-breaking ───────────────────────────────────────

describe("rankSkillsForQuery determinism", () => {
  const tied: RankableSkill[] = [
    { slug: "first", title: "Widget", description: "About buttons and labels." },
    { slug: "second", title: "Widget", description: "About buttons and labels." },
    { slug: "third", title: "Widget", description: "About buttons and labels." },
  ];

  it("breaks score ties by input order", () => {
    expect(rankSkillsForQuery("buttons", tied).slugs).toEqual(["first", "second", "third"]);
  });

  it("breaks ties in reverse when the input order is reversed", () => {
    expect(rankSkillsForQuery("buttons", [...tied].reverse()).slugs).toEqual([
      "third",
      "second",
      "first",
    ]);
  });

  it("scores a section with no body from title and description alone", () => {
    // Eight fillers give N=13-like IDF headroom: with only two documents the
    // maximum IDF is ln(2) ≈ 0.69, below the fallback threshold, and a
    // small-fixture test would measure the fallback, not the scoring.
    const withoutBody: RankableSkill[] = [
      { slug: "color", title: "Color", description: "Contrast as a gate. Apply whenever a colour choice is made." },
      { slug: "typography", title: "Typography", description: "Scale and weights." },
      ...[1, 2, 3, 4, 5, 6].map((n) => ({
        slug: `filler-${n}`,
        title: `Filler number ${n}`,
        description: `Unrelated topic number ${n} carrying several plain filler words.`,
      })),
    ];
    const ranking = rankSkillsForQuery("contrast", withoutBody);
    expect(ranking.fallback).toBe(false);
    expect(ranking.slugs[0]).toBe("color");
  });
});

// ── Tokenizer ──────────────────────────────────────────────────────────

describe("tokenize", () => {
  it("lowercases, splits on non-alphanumerics, drops stopwords and short tokens", () => {
    expect(tokenize("The Button's Contrast!")).toEqual(["button", "contrast"]);
    expect(tokenize("anti-ai-slop defaults")).toEqual(["anti", "ai", "slop", "defaults"]);
    expect(tokenize("it is of the")).toEqual([]);
    expect(tokenize("a b c")).toEqual([]);
    expect(tokenize("")).toEqual([]);
  });
});

// ── Threshold sanity (documents the calibration, not a re-derivation) ──

describe("fallback threshold calibration", () => {
  it("sits between the weakest measured useful query and the vague ones", () => {
    // Measured best scores on the real corpus:
    //   useful:   "animate the drawer opening" 1.181 (weakest useful),
    //             "the icon is too small" 1.744, everything else ≥ 2.1
    //   vague:    "make it prettier" 0.276, "cards everywhere with shadows"
    //             0.658 (ranked color first if admitted — wrong, so
    //             fallback is the better outcome), no-match queries 0.000
    // 0.75 keeps every measured useful query in and both vague queries
    // out. It cannot separate "improve the design" (1.192, ranks layout)
    // from the useful 1.181 — that pair is beyond an absolute threshold
    // and is documented as a limitation, not fixed by a magic number.
    expect(FALLBACK_SCORE_THRESHOLD).toBeGreaterThan(0.7);
    expect(FALLBACK_SCORE_THRESHOLD).toBeLessThan(1.1);

    // Behavioural pin of the two sides of the boundary:
    const corpus = corpusInPriorityOrder();
    expect(rankSkillsForQuery("make it prettier", corpus).fallback).toBe(true);
    expect(rankSkillsForQuery("animate the drawer opening", corpus).fallback).toBe(false);
  });
});
