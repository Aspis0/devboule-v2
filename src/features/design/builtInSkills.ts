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

export function builtInSkillSlugs(): readonly string[] {
  const slugs: string[] = [];
  for (const source of builtInSkillSources()) {
    const result = parseSkillFile(source.path, source.text);
    if (result.ok) slugs.push(result.section.slug);
  }
  return slugs;
}
