// Slash-menu ranking adapted from Paseo's packages/protocol/src/search/text-match.ts and packages/app/src/utils/agent-command-autocomplete.ts.

import type { WorkspaceCommand } from "./WorkspaceCommandMenu";

// Tiers, best first. The word-start tier recognizes the separators a command
// name is built from; a hit anywhere else is the substring tier.
const TIER_EXACT = 0;
const TIER_PREFIX = 1;
const TIER_WORD_START = 2;
const TIER_SUBSTRING = 3;

const SEPARATORS = new Set(["-", "_", ":", ".", "/"]);

interface MatchHit {
  tier: number;
  offset: number;
}

// The best tier across every occurrence, so an early interior hit never hides a later word-start one;
// the strict `<` keeps the earliest offset at that tier.
function bestMatchHit(name: string, query: string): MatchHit | null {
  let best: MatchHit | null = null;
  let from = 0;
  let at = name.indexOf(query, from);
  while (at !== -1) {
    let tier: number;
    if (at === 0 && name.length === query.length) tier = TIER_EXACT;
    else if (at === 0) tier = TIER_PREFIX;
    else if (SEPARATORS.has(name[at - 1])) tier = TIER_WORD_START;
    else tier = TIER_SUBSTRING;
    if (best === null || tier < best.tier) best = { tier, offset: at };
    from = at + 1;
    at = name.indexOf(query, from);
  }
  return best;
}

// Code-unit order on the lowercased name, then the original: a tie-break no runtime locale can reorder.
function compareNames(a: string, b: string): number {
  const aLower = a.toLowerCase();
  const bLower = b.toLowerCase();
  if (aLower !== bLower) return aLower < bLower ? -1 : 1;
  if (a !== b) return a < b ? -1 : 1;
  return 0;
}

/** The slash menu's matches for a query: the substring match set, ordered
 * exact > prefix > word-start > substring, earliest hit first within a tier,
 * ties by name — so `goal` outranks the `goal-*` rows around it. The empty
 * query keeps the source order. */
export function rankCommandMatches(
  commands: readonly WorkspaceCommand[],
  query: string,
): WorkspaceCommand[] {
  if (query === "") return [...commands];
  const needle = query.toLowerCase();
  const scored: { command: WorkspaceCommand; hit: MatchHit }[] = [];
  for (const command of commands) {
    const hit = bestMatchHit(command.name.toLowerCase(), needle);
    if (hit !== null) scored.push({ command, hit });
  }
  scored.sort(
    (a, b) =>
      a.hit.tier - b.hit.tier ||
      a.hit.offset - b.hit.offset ||
      compareNames(a.command.name, b.command.name),
  );
  return scored.map((entry) => entry.command);
}
