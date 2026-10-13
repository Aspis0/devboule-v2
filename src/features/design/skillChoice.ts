// Matched design-craft selection, pure: no React, no I/O, no model calls.
//
// Moved whole from `agentHost.ts` so the daemon-conformance fixtures can run
// the production chooser without the Tauri session machinery; `agentHost`
// re-exports every name, so no caller changes what it imports.

import type { DesignOutputMode } from "./designHost";
import { builtInSkillIndexForOutputMode, MAX_AUTOMATIC_SKILL_SECTIONS } from "./builtInSkills";
import { rankSkillsForQuery } from "./skillRanking";
import craftManifest from "./craft/manifest.json";

// A relevance router structurally cannot select a section whose value is universal:
// that section loses to three sections specific to the request.  This was measured
// three times at 2/15, so automatic mode includes it as a baseline instead.  Keep
// this list very short because every entry spends section budget on every request.
export const AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS: readonly string[] = craftManifest.baseline;
export const MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS =
  MAX_AUTOMATIC_SKILL_SECTIONS - AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS.length;

export function composeAutomaticSkillSlugs(
  routedSlugs: readonly string[],
  allSlugs: readonly string[],
): readonly string[] {
  const known = new Set(allSlugs);
  const seen = new Set<string>();
  const applied: string[] = [];
  for (const slug of [...AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS, ...routedSlugs]) {
    if (!known.has(slug) || seen.has(slug)) continue;
    seen.add(slug);
    applied.push(slug);
  }
  return applied;
}

/**
 * The resolved head of a skill selection, whatever mode produced it: the
 * slugs to compose, in order, and whether the mode's own chooser had to be
 * replaced by the default priority order.
 */
export interface ResolvedSkillChoice {
  slugs: readonly string[];
  fallback: boolean;
}

/**
 * Matched selection: the deterministic lexical ranker orders the corpus for
 * this request, the never-routed baseline is prepended on top, and the head
 * is capped like the automatic mode — same budget arithmetic, no model
 * turn. When the ranker reports no strong match it hands back the priority
 * order and the fallback mirrors the automatic one: request every section
 * and let the composed budget keep the priority head.
 *
 * `outputMode` has no default. The corpus a request may route over is the
 * declared output shape, and an omitted mode would silently mean "page" for
 * a caller that forgot to state it — the same silent substitution this
 * narrowing exists to remove. A missing mode must be a compile error, not a
 * plausible answer.
 */
export function matchSkillChoice(
  prompt: string,
  outputMode: DesignOutputMode,
): ResolvedSkillChoice {
  const index = builtInSkillIndexForOutputMode(outputMode);
  const ranking = rankSkillsForQuery(prompt, index);
  if (ranking.fallback) {
    return { slugs: index.map((entry) => entry.slug), fallback: true };
  }
  const baselineSlugs = new Set<string>(AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS);
  const routed = ranking.slugs
    .filter((slug) => !baselineSlugs.has(slug))
    .slice(0, MAX_AUTOMATIC_ROUTED_SKILL_SECTIONS);
  return {
    slugs: composeAutomaticSkillSlugs(
      routed,
      index.map((entry) => entry.slug),
    ),
    fallback: false,
  };
}
