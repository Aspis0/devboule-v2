import type { SkillSource } from "./skillLoader";

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
