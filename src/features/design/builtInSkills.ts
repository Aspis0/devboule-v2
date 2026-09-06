import { parseSkillFile, type SkillSource } from "./skillLoader";

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
] as const;

const BUILT_IN_SKILL_PRIORITY_INDEX: ReadonlyMap<string, number> = new Map(
  BUILT_IN_SKILL_PRIORITY.map((slug, index) => [slug, index]),
);

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
      });
    }
  }
  return index.sort((left, right) => compareBuiltInSkillSlugs(left.slug, right.slug));
}

// The slug list `buildSkillBlock` wants, derived from the index rather than a
// second parse.  Kept as its own function because composing needs only slugs
// while a picker needs the whole entry.
export function builtInSkillSlugs(): readonly string[] {
  return builtInSkillIndex().map((entry) => entry.slug);
}
