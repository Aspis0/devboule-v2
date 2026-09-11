import type { DesignOutputMode } from "./designHost";
import {
  DOCTRINE_CEILING_CHARS,
  DOCTRINE_SECTION_CEILING_CHARS,
  parseSkillFile,
  SECTION_SEPARATOR,
  type SkillSource,
} from "./skillLoader";

const BUILT_IN_SKILL_FILES = import.meta.glob<string>("./craft/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

// This list is a truncation priority: its head is what survives when the
// budget is refused. A section added without being placed in this list sorts
// last by default, so a new .md file cannot silently displace a listed section.
const BUILT_IN_SKILL_PRIORITY = [
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

const BUILT_IN_SKILL_PRIORITY_INDEX: ReadonlyMap<string, number> = new Map(
  BUILT_IN_SKILL_PRIORITY.map((slug, index) => [slug, index]),
);

/**
 * Sections that make sense for one declared output shape only.
 *
 * The ranker is lexical and the shared vocabulary of design is real: "type",
 * "readable", "contrast", "spacing" name concerns that a page and a deck
 * genuinely share. A request to improve a page therefore scores a deck
 * section on the same words, and the deck section — written densely about
 * legibility, because that is the point of the section — can outrank the
 * page section that actually applies. Tuning words out of the section to
 * quiet the ranker would break on the next craft file and would weaken a
 * section that is right to say those things.
 *
 * The output shape is declared, not inferred (`outputMode` on
 * `DesignGenerationOptions`, on the wire), so the candidate corpus is
 * narrowed by that declaration before the ranker runs. A section absent from
 * this table is available in every mode.
 *
 * This is a table rather than a `slug === "slides"` check inside the chooser
 * on purpose: the next output-specific section is a one-row change here, and
 * no call site can forget the rule. The manual picker keeps the whole
 * catalogue — there the user names the section, so the declaration that
 * would narrow the corpus is the user's own choice and wins.
 */
export const OUTPUT_MODE_SCOPED_SKILL_SLUGS: ReadonlyMap<string, DesignOutputMode> = new Map([
  ["slides", "slides"],
]);

/**
 * Whether a slug competes in this output mode. An unscoped slug always does.
 */
export function isSkillAvailableForOutputMode(slug: string, outputMode: DesignOutputMode): boolean {
  const scope = OUTPUT_MODE_SCOPED_SKILL_SLUGS.get(slug);
  return scope === undefined || scope === outputMode;
}

function compareBuiltInSkillSlugs(left: string, right: string): number {
  const leftPriority = BUILT_IN_SKILL_PRIORITY_INDEX.get(left);
  const rightPriority = BUILT_IN_SKILL_PRIORITY_INDEX.get(right);
  if (leftPriority !== undefined && rightPriority !== undefined) {
    return leftPriority - rightPriority;
  }
  if (leftPriority !== undefined) return -1;
  if (rightPriority !== undefined) return 1;
  return left < right ? -1 : left > right ? 1 : 0;
}

export function builtInSkillSources(): readonly SkillSource[] {
  return Object.entries(BUILT_IN_SKILL_FILES)
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
    .map(([path, text]) => ({ path, text }));
}

export interface BuiltInSkillIndexEntry {
  slug: string;
  title: string;
  description: string;
  body: string;
}

export function builtInSkillIndex(): readonly BuiltInSkillIndexEntry[] {
  const index: BuiltInSkillIndexEntry[] = [];
  for (const source of builtInSkillSources()) {
    const result = parseSkillFile(source.path, source.text);
    if (result.ok) {
      index.push({
        slug: result.section.slug,
        title: result.section.title,
        description: result.section.description,
        body: result.section.body,
      });
    }
  }
  return index.sort((left, right) => compareBuiltInSkillSlugs(left.slug, right.slug));
}

/**
 * The index the chooser may route over: the full catalogue minus the sections
 * scoped to another output mode. Same entries, same order; only the corpus
 * the ranker and the routing prompt see is narrowed.
 */
export function builtInSkillIndexForOutputMode(
  outputMode: DesignOutputMode,
): readonly BuiltInSkillIndexEntry[] {
  return builtInSkillIndex().filter((entry) =>
    isSkillAvailableForOutputMode(entry.slug, outputMode),
  );
}

const MAX_BUILT_IN_SKILL_TITLE_CHARS = Math.max(
  0,
  ...builtInSkillIndex().map((entry) => entry.title.length),
);
const MAX_SKILL_BLOCK_CHARS =
  DOCTRINE_SECTION_CEILING_CHARS + "## ".length + MAX_BUILT_IN_SKILL_TITLE_CHARS + "\n\n".length;
const MAX_SKILL_BLOCK_WITH_SEPARATOR_CHARS = MAX_SKILL_BLOCK_CHARS + SECTION_SEPARATOR.length;

/**
 * Shared cap for automatic and manual selection. It is derived from the
 * worst-case built-in block: section ceiling + longest title header + the
 * separator between blocks. Four fit under the composed ceiling; five do not.
 */
export const MAX_AUTOMATIC_SKILL_SECTIONS = Math.floor(
  (DOCTRINE_CEILING_CHARS + SECTION_SEPARATOR.length) / MAX_SKILL_BLOCK_WITH_SEPARATOR_CHARS,
);

// The slug list `buildSkillBlock` wants, derived from the index rather than a
// second parse.  Kept as its own function because composing needs only slugs
// while a picker needs the whole entry.
export function builtInSkillSlugs(): readonly string[] {
  return builtInSkillIndex().map((entry) => entry.slug);
}
