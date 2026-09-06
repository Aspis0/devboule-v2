import { parseSkillFile, type SkillSource } from "./skillLoader";

const BUILT_IN_SKILL_FILES = import.meta.glob<string>("./craft/*.md", {
  query: "?raw",
  import: "default",
  eager: true,
});

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
  return index;
}

// The slug list `buildSkillBlock` wants, derived from the index rather than a
// second parse.  Kept as its own function because composing needs only slugs
// while a picker needs the whole entry.
export function builtInSkillSlugs(): readonly string[] {
  return builtInSkillIndex().map((entry) => entry.slug);
}
