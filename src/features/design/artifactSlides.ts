/*
 * The slides contract, read back from the artifact that came out of it.
 *
 * In slides mode the prompt asks for a specific shape (`agentHost.ts`): "a single
 * self-contained HTML document that renders a slide deck: one <section> per slide,
 * each with a stable id (id="slide-1", id="slide-2", ...)". The prompt gives its own
 * reason, and the reason is why the shape is not cosmetic: the id is the note anchor,
 * so a section without one anchors to document position, and a regeneration detaches
 * every note from its slide.
 *
 * Nothing read that back. The mode changed the prompt and the available craft sections,
 * and then the reply was rendered exactly as page output is — same parser, same frame,
 * same CSS — so a run that returned one undivided blob with no <section> at all was
 * indistinguishable, to the code, from a correct deck. This module is the reading-back.
 *
 * WHAT THIS IS NOT: a gate.
 *
 * It never rejects, rewrites, rewrites around, or withholds the artifact. A deck that
 * breaks the contract is still the user's result, and the user must still see it exactly
 * as it was generated — the check reports the shape it found, and the caller decides
 * whether anything is said about it. The temptation to make this a gate ("hold the
 * artifact until it has sections") is the mistake this paragraph exists to stop. The
 * rendering path already works for whatever markup arrives, including markup that
 * ignores every instruction in the prompt; a check that starts refusing it would turn a
 * notice into a broken generation.
 *
 * WHY THIS READS THE HTML AND NOT THE MEASURED STRUCTURE
 *
 * `artifactStructure.ts` is the module that already reads artifact structure, and the
 * obvious move is to reuse it. It cannot be reused here, and the reason is a failure
 * mode rather than taste:
 *
 * - Its reader is the frame-side collector `collectArtifactStructure`, whose source is
 *   interpolated into the critic's measurement script and which runs inside a hidden
 *   iframe, not here. It exists only after a render pass.
 * - That collector is layout-gated on purpose: an element with a missing, empty or
 *   non-finite box is skipped, and so is one whose computed `display` is `none`. Those
 *   are the right rules for hit zones — you cannot click a box that is not there — and
 *   the wrong rules for this question. A slide hidden by a stylesheet, or one that
 *   measured 0 px high because the measurement pass ran before a font arrived, is still
 *   a section the prompt asked for, and dropping it here would invent a defect.
 * - The measurement can fail. A timeout, a rejected message, one entry outside the
 *   protocol's bounds, and the whole index is dropped to nothing. Every failure of that
 *   path would then be reported to the user as "this deck has no sections", which is the
 *   expensive kind of wrong: a notice on correct output teaches the reader to dismiss the
 *   notice, and then the real finding is invisible too.
 *
 * So this reads the artifact text, synchronously, before anything is rendered, and it
 * says nothing about where the sections are or how they look. What it shares with the
 * structure module is the parser, not the policy: `DOMParser` with `text/html`, which is
 * the same parser that renders the artifact and the one `artifactSave.ts` and
 * `designAttachments.ts` already use to read artifact-side markup.
 *
 * WHY A PARSER AND NOT A REGEX
 *
 * The same argument `designAttachments.ts` makes for its SVG pass applies here: a
 * pattern would have to model HTML's own syntax, and it would be wrong in exactly the
 * cases that matter. Measured in happy-dom, which is the environment the tests run in:
 * a `<section>` inside an HTML comment, inside a `<script>` string, inside an attribute
 * value, or inside a `<template>` is not counted by the parser and would be counted by a
 * substring search; an uppercase `<SECTION>` is counted by the parser and missed by a
 * search for the lowercase tag. The parser in front of us is the one that will render
 * the result, so it is the one whose answer is worth reporting.
 *
 * MODE IS THE CALLER'S BUSINESS
 *
 * Nothing here takes an output mode. The mode is a property of the request, not of the
 * document, and a function of the artifact text cannot become wrong when the toggle next
 * to it is set to page. A page-mode artifact is allowed to contain `<section>` landmarks
 * and usually does; running this check on one produces a statement about a contract that
 * never applied — not a noise, a falsehood. So the caller gates on the mode the run
 * recorded on the artifact it produced — never on the switch beside the canvas, which
 * states what the *next* run will ask for, and would accuse the artifact on screen of a
 * contract it was never given, or drop a real finding when the switch moves the other
 * way:
 *
 *   const notice =
 *     producedMode === "slides" ? artifactSlideNotice(readArtifactSlideShape(html)) : "";
 *
 * `producedMode` is absent for a run that recorded none — a message restored from a
 * document saved before the field existed, or a host that did not report one. Absent
 * fails the gate silently, which is the point: a mode nobody can show was asked for is
 * not `page`.
 *
 * `artifactSlideNotice` returns the empty string when there is nothing to report, which is
 * the convention the other notice builders in this feature follow (`svgSanitizerNotice`,
 * `rasterMetadataNotice`), so "no message" cannot be confused with a failure to compute one.
 */

/**
 * The prefix every slide id carries: `slide-1`, `slide-2`, ... The contract in
 * `agentHost.ts` names the ids in exactly this form, so this is the single place that
 * spells it, and the check cannot drift from the prompt without editing a constant.
 */
export const SLIDE_ID_PREFIX = "slide-";

/** Longest list of id values quoted in one sentence before it is summarized. */
const MAX_QUOTED_IDS = 3;

/**
 * What the prompt asked for, resolved to the facts the report carries. The verdict is
 * derived from the counts and lists beside it by the fixed precedence documented on
 * `ArtifactSlideShape.verdict`, never computed on its own, so it cannot disagree with
 * them: it is an index into the report, not a second opinion about it.
 */
export type ArtifactSlideVerdict =
  /** Sections, every one with an id, and the ids are `slide-1` .. `slide-N` in order. */
  | "matches"
  /** No `<section>` element at all: the reply is one undivided document. */
  | "no-sections"
  /** At least one section carries no usable `id` attribute. */
  | "missing-ids"
  /** Every section has an id, but an id names more than one section. */
  | "duplicate-ids"
  /** Every section has a unique id, but the ids are not `slide-1` .. `slide-N` in order. */
  | "off-sequence";

/**
 * What the artifact looks like against the slides contract. Facts, not a sentence: the
 * caller reads the count and the lists and can say more than the verdict does — "3
 * sections, none with an id" is a different (and more useful) report than "not a deck".
 * `artifactSlideNotice` is the ready-made sentence, and a caller that wants different
 * words has everything here it needs to write them.
 */
export interface ArtifactSlideShape {
  /** Number of `<section>` elements in document order. */
  readonly sectionCount: number;
  /**
   * The `id` of each section in document order, trimmed, with `""` where the attribute
   * is absent, empty, or whitespace. Trimming matches the structure module, which trims
   * the attribute before using it as an anchor, so `id=" slide-1 "` reads as usable here
   * exactly as it does there. The comparison is otherwise case-sensitive: `Slide-1` is
   * not `slide-1`, and the id is an anchor, which is.
   */
  readonly ids: readonly string[];
  /** How many sections carry no usable id. */
  readonly sectionsWithoutId: number;
  /**
   * Ids carried by more than one section, deduplicated, in order of first appearance.
   *
   * Reported apart from the off-sequence list because the consequence is worse than a
   * naming mismatch: the structure collector keeps a `seen` set of anchors and skips an
   * element whose anchor it has already collected, so the second section with an id is
   * not indexed at all — it loses its layer and cannot hold a note. That is worth its
   * own field, and its own sentence.
   */
  readonly duplicateIds: readonly string[];
  /**
   * Ids that are present and do not match the id expected at their position. Only
   * non-empty ids appear: a section with no id is reported by `sectionsWithoutId`, and
   * listing `""` here as though the user had typed it would be a report about nothing.
   */
  readonly offSequenceIds: readonly string[];
  /**
   * What the contract asks for at each position: `slide-1` .. `slide-N` for the sections
   * this artifact actually has. Derived from `sectionCount`, not from the ids found, so
   * it cannot inherit their mistakes.
   */
  readonly expectedIds: readonly string[];
  /**
   * The report in one word, by fixed precedence: `no-sections`, then `missing-ids`, then
   * `duplicate-ids`, then `off-sequence`, then `matches`. The order is coarsest first —
   * "there are no sections" is a different problem from "the sections are not numbered
   * the way the prompt asked". The lists above are not truncated by this choice: an
   * artifact that is both missing and duplicating ids reports enough for a caller to say
   * both, and `artifactSlideNotice` names the highest-precedence one.
   */
  readonly verdict: ArtifactSlideVerdict;
}

/** The id the contract asks for at a 1-based slide position. */
function slideIdAt(position: number): string {
  return `${SLIDE_ID_PREFIX}${position}`;
}

/**
 * `a`, `a and b`, `a, b and c`: a conjunction before the last item, never a stray one.
 * The same list rule the attachment notices use.
 */
function listWithAnd(items: readonly string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * The id values quoted for a sentence, bounded. A deck that ignored every naming rule
 * can carry thirty ids, and a notice that prints all of them is a wall the user scrolls
 * past instead of a sentence they read: the first few name the pattern, the rest are
 * counted.
 */
function quotedIds(ids: readonly string[]): string {
  const quoted = ids.slice(0, MAX_QUOTED_IDS).map((id) => `"${id}"`);
  if (ids.length <= MAX_QUOTED_IDS) return listWithAnd(quoted);
  return `${quoted.join(", ")} and ${ids.length - MAX_QUOTED_IDS} more`;
}

function sectionNoun(count: number): string {
  return count === 1 ? "1 section" : `${count} sections`;
}

/**
 * Reads the artifact against the slides contract. Pure over the text: no render, no
 * measurement, no state, and the same answer before and after the artifact is shown.
 *
 * `<section>` elements are counted at every depth, including a section nested inside
 * another. The prompt says one section per slide and says nothing about grouping, so a
 * nested section is a second section that the anchor scheme will treat as its own slide
 * anchor; counting the outer one only would hide exactly the case where the deck and the
 * note anchors disagree about how many slides there are.
 *
 * It cannot fail, and that is the point: a malformed document, an empty string, and plain
 * text that is not markup all produce a report, because a check that can return "no
 * answer" would have to be told apart from "no sections". The one thing that would throw
 * is a missing `DOMParser`, which means the caller is not in a document environment at
 * all, and returning a shape there would be inventing an answer rather than reporting one.
 */
export function readArtifactSlideShape(html: string): ArtifactSlideShape {
  const parsed = new DOMParser().parseFromString(html, "text/html");
  const sections = parsed.querySelectorAll("section");

  const ids: string[] = [];
  for (const section of sections) {
    ids.push((section.getAttribute("id") ?? "").trim());
  }

  const expectedIds = ids.map((_, index) => slideIdAt(index + 1));
  const sectionsWithoutId = ids.filter((id) => id === "").length;

  const occurrences = new Map<string, number>();
  for (const id of ids) {
    if (id === "") continue;
    occurrences.set(id, (occurrences.get(id) ?? 0) + 1);
  }
  const duplicateIds: string[] = [];
  for (const id of ids) {
    if (id === "" || duplicateIds.includes(id)) continue;
    if ((occurrences.get(id) ?? 0) > 1) duplicateIds.push(id);
  }

  const offSequenceIds = ids.filter((id, index) => id !== "" && id !== expectedIds[index]);

  let verdict: ArtifactSlideVerdict = "matches";
  if (ids.length === 0) verdict = "no-sections";
  else if (sectionsWithoutId > 0) verdict = "missing-ids";
  else if (duplicateIds.length > 0) verdict = "duplicate-ids";
  else if (offSequenceIds.length > 0) verdict = "off-sequence";

  return {
    sectionCount: ids.length,
    ids,
    sectionsWithoutId,
    duplicateIds,
    offSequenceIds,
    expectedIds,
    verdict,
  };
}

/**
 * What the prompt asked for at the positions this artifact has: `the id slide-1` for a
 * single section, `the ids slide-1 to slide-3` for several. Written out rather than
 * assembled in the sentence so the singular case cannot read "the ids slide-1 to slide-1".
 */
function askedIds(shape: ArtifactSlideShape): string {
  const first = shape.expectedIds[0] ?? "";
  const last = shape.expectedIds[shape.expectedIds.length - 1] ?? first;
  if (shape.sectionCount <= 1) return `the id ${first}`;
  return `the ids ${first} to ${last} in order`;
}

const ASKED_SECTIONS = "one <section> per slide";
const ASKED_IDS = 'an id="slide-N" on every slide';

/**
 * One sentence about an artifact that does not have the shape slides mode asked for, or
 * the empty string when it does — the convention `svgSanitizerNotice` and
 * `rasterMetadataNotice` follow, so "nothing to say" is a value the caller can render
 * without asking a second question.
 *
 * The sentence leads with the contract and follows with the count it found, because a
 * user who has just been told "3 sections, none with an id" still does not know what was
 * wanted. It reports and stops there: no instruction to regenerate, no claim about how
 * the deck looks, because a `<div class="slide">` deck with scroll snapping can look
 * correct and still break every note anchor, and the reverse is also true.
 */
export function artifactSlideNotice(shape: ArtifactSlideShape): string {
  if (shape.verdict === "matches") return "";

  if (shape.verdict === "no-sections") {
    return `Slides mode asked for ${ASKED_SECTIONS}; this artifact has no <section> elements.`;
  }

  const noun = sectionNoun(shape.sectionCount);

  if (shape.verdict === "missing-ids") {
    // "none with an id" is the phrasing that carries a count without a verb, so no
    // subject–verb agreement can go wrong — except at one section, where "none" is a
    // crowd of one and the same clause is better said plainly.
    const missing =
      shape.sectionsWithoutId !== shape.sectionCount
        ? `${shape.sectionsWithoutId} with no id`
        : shape.sectionCount === 1
          ? "with no id"
          : "none with an id";
    return `Slides mode asked for ${ASKED_IDS}; this artifact has ${noun}, ${missing}.`;
  }

  if (shape.verdict === "duplicate-ids") {
    const subject =
      shape.duplicateIds.length === 1
        ? `the id ${quotedIds(shape.duplicateIds)} appears`
        : `the ids ${quotedIds(shape.duplicateIds)} appear`;
    return `Slides mode asked for ${ASKED_IDS}; this artifact has ${noun}, and ${subject} on more than one of them.`;
  }

  // A single section is addressed directly: "1 of them" reads as a crowd of one. The
  // ids are quoted only where they differ from the ones asked, because the counts are
  // what say how far off the deck is, and quoting an id that was never wrong would
  // invite the reader to hunt for a problem in a section that has none.
  if (shape.sectionCount === 1) {
    return `Slides mode asked for ${askedIds(shape)}; this artifact has ${noun}, and its id is ${quotedIds(shape.offSequenceIds)}.`;
  }
  const offCount = shape.offSequenceIds.length;
  const subject =
    offCount === 1
      ? `1 of them carries a different id (${quotedIds(shape.offSequenceIds)})`
      : `${offCount} of them carry different ids (${quotedIds(shape.offSequenceIds)})`;
  return `Slides mode asked for ${askedIds(shape)}; this artifact has ${noun}, and ${subject}.`;
}
