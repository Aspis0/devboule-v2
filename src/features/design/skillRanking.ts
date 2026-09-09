// ── Deterministic lexical skill selector ───────────────────────────────
//
// Ranks design-doctrine skill sections against a user query with plain
// BM25 over field-weighted term frequencies (a flattened BM25F: field
// weights scale term frequencies and document lengths, then the ordinary
// BM25 formula runs once over the whole vocabulary). Pure: no React, no
// I/O, no npm dependencies, no model calls. Same input, same output.
//
// Responsibilities end at ordering. The always-included baseline
// (`anti-ai-slop`) and the composed-budget truncation (~4 sections) are
// the caller's decisions; this module returns the complete ranking.
//
// Field weights (chosen, not invented — see rationale beside each
// constant). BM25 parameters:
//   k1 = 1.5 — term-frequency saturation. Standard Robertson/Sparck Jones
//     range is 1.2–2.0; 1.5 is the common mid-range pick (also the Lucene
//     default). With tf already diluted by field weights, lower saturation
//     is harmless; higher would let a term repeated in one long body
//     dominate.
//   b = 0.75 — document-length normalization. Standard value; 0 would
//     disable normalization (long bodies win on raw tf), 1 normalizes
//     fully and over-rewards short fields. 0.75 is the standard partial
//     normalization.
//   IDF uses the non-negative variant used by Lucene and most
//   implementations: ln(1 + (N - df + 0.5) / (df + 0.5)).

export interface RankableSkill {
  slug: string;
  title: string;
  description: string;
  body?: string;
}

export interface SkillRanking {
  /** All slugs, best first. Complete ranking, not a truncated head. */
  slugs: readonly string[];
  /**
   * True when the query produced no useful tokens (empty or stopword-only)
   * or the best BM25 score is below `FALLBACK_SCORE_THRESHOLD`. The slug
   * list is then the input order, untouched: the input order is the
   * priority order, so refusing to rank means keeping it.
   */
  fallback: boolean;
}

// Field weights — see the rationale in the comment block below.
// The descriptions are written as explicit applicability clauses
// ("Apply whenever …"), the curated relevance signal for this corpus, so
// they lead. Titles are two or three dense tokens — highly informative per
// token, but few; a heavy weight would let one lucky title match outweigh a
// description full of matches, and BM25's length normalization already
// boosts short documents. Bodies are long (~400 tokens) and diluted with
// cited-source vocabulary (Carbon, WCAG, Material, GOV.UK, URL fragments)
// that is not about the section's topic; the body corroborates the
// description, it does not lead. Exported so tests can reference them
// without magic numbers.
export const W_TITLE = 0.3;
export const W_DESCRIPTION = 0.5;
export const W_BODY = 0.2;

// Saturation and length-normalization constants, rationale in the file
// header. Exported so tests and callers can read them without magic
// numbers.
export const K1 = 1.5;
export const B = 0.75;

// Below this best-document BM25 score the query is treated as too weak to
// rank on: the ranking would be a coin toss between near-zero scores, and
// the input priority order is the safer output. Calibrated by hand on the
// real 13-section corpus — useful one-topic queries land well above it,
// vague queries ("make it prettier") and no-match queries land below it.
// Exported for tests; the value and its calibration are documented in
// skillRanking.test.ts.
export const FALLBACK_SCORE_THRESHOLD = 0.75;

// Minimal English stopword list. The sections are English; the list is
// deliberately small because BM25's IDF already demotes ubiquitous terms —
// removing them up front just saves arithmetic. Kept in one place, sorted.
const STOPWORDS: ReadonlySet<string> = new Set([
  "a", "all", "an", "and", "any", "are", "as", "at", "be", "been", "being",
  "both", "but", "by", "can", "cannot", "could", "did", "do", "does", "for",
  "from", "had", "has", "have", "he", "her", "here", "hers", "him", "his",
  "how", "i", "if", "in", "into", "is", "it", "its", "me", "might", "must",
  "my", "no", "not", "now", "of", "on", "only", "or", "other", "our", "out",
  "over", "she", "should", "so", "some", "such", "than", "that", "the",
  "their", "them", "then", "there", "these", "they", "this", "those", "to",
  "too", "up", "us", "was", "we", "were", "what", "when", "which", "who",
  "whom", "why", "will", "with", "would", "you", "your",
]);

// Tokens shorter than this are dropped: single letters come from split
// artifacts (accented characters, initials) and carry no topical signal.
const MIN_TOKEN_LENGTH = 2;

/**
 * Tokenize: lowercase, split on runs of non-alphanumeric characters,
 * drop stopwords and tokens shorter than `MIN_TOKEN_LENGTH`.
 *
 * Deliberately simple and documented rather than clever:
 * - No stemming. "buttons" does not match "button"; queries that stem
 *   badly ("designing" → "design") are rare here and a crude stemmer
 *   corrupts real words ("this" → "thi"). The descriptions use both the
 *   singular and plural of their key terms, so the loss is small.
 * - No URL stripping. Section bodies cite sources by URL, so tokens like
 *   "carbon", "developer" or "w3" enter the index. They appear in many
 *   documents, so IDF demotes them to noise; a URL-stripping regex would
 *   add fragility for no measurable gain.
 * - Accented characters split ("café" → "caf", "e"); the corpus is
 *   English, so the artifact tokens are short enough to be filtered.
 */
export function tokenize(text: string): readonly string[] {
  return text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter((token) => token.length >= MIN_TOKEN_LENGTH && !STOPWORDS.has(token));
}

// A document as the ranker sees it: one term → weighted-frequency map plus
// the weighted token count. "Weighted" means field weights have been
// applied to the raw term frequencies and to the length, which is what
// lets the single BM25 formula below act on all three fields at once
// (flattened BM25F: within-field saturation is ignored — defensible here
// because two of the three fields are too short to saturate).
interface WeightedDocument {
  tf: ReadonlyMap<string, number>;
  length: number;
}

interface ScoredIndex {
  docs: readonly WeightedDocument[];
  avgLength: number;
  documentFrequency: ReadonlyMap<string, number>;
}

function buildScoredIndex(index: readonly RankableSkill[]): ScoredIndex {
  const docs: WeightedDocument[] = [];
  const documentFrequency = new Map<string, number>();
  let totalLength = 0;

  for (const skill of index) {
    const tf = new Map<string, number>();
    let length = 0;
    const fields: ReadonlyArray<readonly [string, number]> = [
      [skill.title, W_TITLE],
      [skill.description, W_DESCRIPTION],
      [skill.body ?? "", W_BODY],
    ];
    for (const [text, weight] of fields) {
      for (const token of tokenize(text)) {
        tf.set(token, (tf.get(token) ?? 0) + weight);
        length += weight;
      }
    }
    docs.push({ tf, length });
    totalLength += length;
    for (const token of new Set(tf.keys())) {
      documentFrequency.set(token, (documentFrequency.get(token) ?? 0) + 1);
    }
  }

  const avgLength = index.length > 0 ? totalLength / index.length : 1;
  return { docs, avgLength, documentFrequency };
}

function idf(documentFrequency: number, documentCount: number): number {
  return Math.log(1 + (documentCount - documentFrequency + 0.5) / (documentFrequency + 0.5));
}

/**
 * Rank skill sections for a query. Deterministic: identical inputs produce
 * an identical ranking, and equal scores break by input order (the input
 * order is the priority order, so it must not be lost on a tie — with 13
 * documents and short queries, ties are frequent).
 */
export function rankSkillsForQuery(query: string, index: readonly RankableSkill[]): SkillRanking {
  const inputOrder = index.map((skill) => skill.slug);

  // Ripiego 1: no useful tokens — empty, whitespace, stopword-only. Rank
  // nothing; hand back the input order and say so.
  const queryTokens = tokenize(query);
  if (index.length === 0 || queryTokens.length === 0) {
    return { slugs: inputOrder, fallback: true };
  }

  const { docs, avgLength, documentFrequency } = buildScoredIndex(index);
  const documentCount = docs.length;
  // Repeated query tokens count once per distinct token: a duplicated word
  // in the query is noise, not double evidence.
  const distinctTokens = [...new Set(queryTokens)];

  const scores = docs.map((doc) => {
    let score = 0;
    for (const token of distinctTokens) {
      const tf = doc.tf.get(token);
      if (tf === undefined) continue;
      const frequency = documentFrequency.get(token) ?? 0;
      score += idf(frequency, documentCount) * ((tf * (K1 + 1)) / (tf + K1 * (1 - B + B * (doc.length / avgLength))));
    }
    return score;
  });

  const best = scores.length > 0 ? Math.max(...scores) : 0;

  // Ripiego 2: best score under threshold — the query matched nothing or
  // only ubiquitous/diluted tokens, so any ordering would be arbitrary.
  // Hand back the input order and say so.
  if (best < FALLBACK_SCORE_THRESHOLD) {
    return { slugs: inputOrder, fallback: true };
  }

  const ranked = index
    .map((skill, position) => ({ slug: skill.slug, score: scores[position], position }))
    .sort((left, right) => right.score - left.score || left.position - right.position)
    .map((entry) => entry.slug);

  return { slugs: ranked, fallback: false };
}
