// @vitest-environment happy-dom

/*
 * The check reads markup with the same parser that renders it, so it needs a document
 * environment — the header above and `happy-dom` are what the rest of the feature's
 * artifact tests already use (`artifactStructure.test.ts`, `artifactSave.test.ts`).
 * Nothing here mounts a component or measures a box: the function is pure over the
 * artifact text, and every case below is a string in and a sentence out.
 */

import { describe, expect, it } from "vitest";
import {
  SLIDE_ID_PREFIX,
  artifactSlideNotice,
  readArtifactSlideShape,
  type ArtifactSlideShape,
} from "./artifactSlides";

/** `deckHtml(["slide-1", "slide-2"])`; an empty id means no id attribute at all. */
function deckHtml(ids: readonly string[]): string {
  return ids
    .map(
      (id, index) =>
        `<section${id === "" ? "" : ` id="${id}"`}><h2>Slide ${index + 1}</h2><p>One idea</p></section>`,
    )
    .join("");
}

describe("the deck the prompt asked for", () => {
  it("accepts one section per slide, numbered slide-1..slide-N in order", () => {
    const shape = readArtifactSlideShape(deckHtml(["slide-1", "slide-2", "slide-3"]));

    expect(shape).toEqual({
      sectionCount: 3,
      ids: ["slide-1", "slide-2", "slide-3"],
      sectionsWithoutId: 0,
      duplicateIds: [],
      offSequenceIds: [],
      expectedIds: ["slide-1", "slide-2", "slide-3"],
      verdict: "matches",
    });
    // Nothing to say is an empty string, not null and not a reassurance: the callers of
    // the sibling notices (`svgSanitizerNotice`, `rasterMetadataNotice`) render this
    // straight into the surface, and "" is how they say nothing happened.
    expect(artifactSlideNotice(shape)).toBe("");
  });

  it("reads the same shape from a full document and from a bare fragment", () => {
    const fragment = readArtifactSlideShape(deckHtml(["slide-1", "slide-2"]));
    const document = readArtifactSlideShape(
      `<!doctype html><html><head><title>Q3</title><style>section{height:720px}</style></head>` +
        `<body>${deckHtml(["slide-1", "slide-2"])}</body></html>`,
    );
    expect(document).toEqual(fragment);
  });

  it("trims the id the way the anchor scheme does, and counts a blank one as absent", () => {
    const trimmed = readArtifactSlideShape('<section id="  slide-1  ">a</section>');
    expect(trimmed.ids).toEqual(["slide-1"]);
    expect(trimmed.verdict).toBe("matches");

    const blank = readArtifactSlideShape('<section id="   ">a</section>');
    expect(blank.ids).toEqual([""]);
    expect(blank.sectionsWithoutId).toBe(1);
    expect(blank.verdict).toBe("missing-ids");
  });

  it("takes the id from the section, not from anything inside it", () => {
    // A slide whose wrapper div carries the id is the shape that reads correct in a
    // diff and wrong in the Layers panel: the div is not a section, so the anchor the
    // notes resolve is not the one the prompt asked for.
    const shape = readArtifactSlideShape('<section><div id="slide-1">Title</div></section>');
    expect(shape.ids).toEqual([""]);
    expect(shape.offSequenceIds).toEqual([]);
    expect(shape.verdict).toBe("missing-ids");
  });

  it("is case-sensitive about the id, which is an anchor", () => {
    const shape = readArtifactSlideShape(deckHtml(["Slide-1", "SLIDE-2"]));
    expect(shape.verdict).toBe("off-sequence");
    expect(shape.offSequenceIds).toEqual(["Slide-1", "SLIDE-2"]);
  });
});

describe("decks that do not match the contract", () => {
  it("reports a document with no sections at all", () => {
    const shape = readArtifactSlideShape(
      '<div class="deck"><h1>Q3 review</h1><p>Revenue, churn and the roadmap.</p></div>',
    );
    expect(shape.sectionCount).toBe(0);
    expect(shape.ids).toEqual([]);
    expect(shape.expectedIds).toEqual([]);
    expect(shape.verdict).toBe("no-sections");
    expect(artifactSlideNotice(shape)).toBe(
      "Slides mode asked for one <section> per slide; this artifact has no <section> elements.",
    );
  });

  it("reports sections with none of them carrying an id", () => {
    const shape = readArtifactSlideShape(deckHtml(["", "", ""]));
    expect(shape.sectionsWithoutId).toBe(3);
    expect(shape.verdict).toBe("missing-ids");
    expect(artifactSlideNotice(shape)).toBe(
      'Slides mode asked for an id="slide-N" on every slide; this artifact has 3 sections, none with an id.',
    );
  });

  it("reports the one-section case without a noun that disagrees with it", () => {
    const shape = readArtifactSlideShape(deckHtml([""]));
    expect(shape.verdict).toBe("missing-ids");
    expect(artifactSlideNotice(shape)).toBe(
      'Slides mode asked for an id="slide-N" on every slide; this artifact has 1 section, with no id.',
    );
  });

  it("reports a partly id'd deck by how many sections are missing one", () => {
    const shape = readArtifactSlideShape(
      deckHtml(["slide-1", "slide-2", "", "slide-4", "", "slide-6"]),
    );
    expect(shape.sectionsWithoutId).toBe(2);
    // The two blanks are not listed as off-sequence ids: the user typed no id there, and
    // reporting "" among the ids would be a report about nothing.
    expect(shape.offSequenceIds).toEqual([]);
    expect(shape.verdict).toBe("missing-ids");
    expect(artifactSlideNotice(shape)).toBe(
      'Slides mode asked for an id="slide-N" on every slide; this artifact has 6 sections, 2 with no id.',
    );
  });

  it("reports ids that are present but are not the sequence", () => {
    const shape = readArtifactSlideShape(deckHtml(["cover", "agenda", "close"]));
    expect(shape.expectedIds).toEqual(["slide-1", "slide-2", "slide-3"]);
    expect(shape.duplicateIds).toEqual([]);
    expect(shape.verdict).toBe("off-sequence");
    expect(artifactSlideNotice(shape)).toBe(
      "Slides mode asked for the ids slide-1 to slide-3 in order; this artifact has 3 sections, " +
        'and 3 of them carry different ids ("cover", "agenda" and "close").',
    );
  });

  it("reports ids that are in the wrong order", () => {
    const shape = readArtifactSlideShape(deckHtml(["slide-2", "slide-1"]));
    expect(shape.sectionsWithoutId).toBe(0);
    expect(shape.offSequenceIds).toEqual(["slide-2", "slide-1"]);
    expect(shape.verdict).toBe("off-sequence");
    expect(artifactSlideNotice(shape)).toBe(
      "Slides mode asked for the ids slide-1 to slide-2 in order; this artifact has 2 sections, " +
        'and 2 of them carry different ids ("slide-2" and "slide-1").',
    );
  });

  it("reports a gap in the sequence and quotes only the id that is wrong", () => {
    const shape = readArtifactSlideShape(deckHtml(["slide-1", "slide-3"]));
    // slide-1 is at the position the contract asks it to be, so it is not named.
    expect(shape.offSequenceIds).toEqual(["slide-3"]);
    expect(shape.verdict).toBe("off-sequence");
    expect(artifactSlideNotice(shape)).toBe(
      "Slides mode asked for the ids slide-1 to slide-2 in order; this artifact has 2 sections, " +
        'and 1 of them carries a different id ("slide-3").',
    );
  });

  it("reports an id that names more than one section", () => {
    const shape = readArtifactSlideShape(deckHtml(["slide-1", "slide-1"]));
    expect(shape.sectionsWithoutId).toBe(0);
    expect(shape.duplicateIds).toEqual(["slide-1"]);
    // The second slide-1 is at the position that asks for slide-2, so it is off-sequence
    // as well — but the duplicate is the fact worth leading with: the structure collector
    // keeps one anchor per id and skips the second element, so that section loses its
    // layer and cannot hold a note.
    expect(shape.offSequenceIds).toEqual(["slide-1"]);
    expect(shape.verdict).toBe("duplicate-ids");
    expect(artifactSlideNotice(shape)).toBe(
      'Slides mode asked for an id="slide-N" on every slide; this artifact has 2 sections, ' +
        'and the id "slide-1" appears on more than one of them.',
    );
  });

  it("names several duplicated ids once each", () => {
    const shape = readArtifactSlideShape(deckHtml(["a", "a", "b", "b"]));
    expect(shape.duplicateIds).toEqual(["a", "b"]);
    expect(shape.verdict).toBe("duplicate-ids");
    expect(artifactSlideNotice(shape)).toBe(
      'Slides mode asked for an id="slide-N" on every slide; this artifact has 4 sections, ' +
        'and the ids "a" and "b" appear on more than one of them.',
    );
  });

  it("keeps the sentence readable when a deck ignored the naming rule entirely", () => {
    const shape = readArtifactSlideShape(deckHtml(["a", "b", "c", "d", "e"]));
    expect(shape.offSequenceIds).toEqual(["a", "b", "c", "d", "e"]);
    expect(artifactSlideNotice(shape)).toBe(
      "Slides mode asked for the ids slide-1 to slide-5 in order; this artifact has 5 sections, " +
        'and 5 of them carry different ids ("a", "b", "c" and 2 more).',
    );
  });

  it("counts a nested section as a section", () => {
    // The prompt asks for one section per slide and says nothing about grouping, and the
    // anchor scheme gives the inner section its own slide anchor. Counting the outer one
    // only would hide the case where the deck and the note anchors disagree about how
    // many slides there are.
    const shape = readArtifactSlideShape(
      `<section id="slide-1"><section id="slide-1-inner">Nested</section></section>`,
    );
    expect(shape.sectionCount).toBe(2);
    expect(shape.ids).toEqual(["slide-1", "slide-1-inner"]);
    expect(shape.verdict).toBe("off-sequence");
  });

  it("answers for text that is empty or is not markup, without throwing", () => {
    // A check that can return "no answer" would have to be told apart from "no sections";
    // this one reports, so a failed extraction and a one-blob reply read the same way —
    // which is honest, because from the artifact's point of view they are the same.
    for (const input of [
      "",
      "   ",
      "not markup at all",
      "<!doctype html><html><body></body></html>",
    ]) {
      const shape = readArtifactSlideShape(input);
      expect(shape.sectionCount).toBe(0);
      expect(shape.verdict).toBe("no-sections");
      expect(artifactSlideNotice(shape)).toContain("no <section> elements");
    }
  });

  it("prefixes every expected id with the constant the prompt and the check share", () => {
    const shape: ArtifactSlideShape = readArtifactSlideShape(deckHtml(["slide-1"]));
    expect(SLIDE_ID_PREFIX).toBe("slide-");
    expect(shape.expectedIds).toEqual([`${SLIDE_ID_PREFIX}1`]);
  });
});

describe("what the parser decides and a tag search would not", () => {
  it("counts no section the renderer will not build", () => {
    const shape = readArtifactSlideShape(
      [
        '<!-- <section id="slide-1">A commented-out slide</section> -->',
        `<script>const template = '<section id="slide-2">string, not markup</section>';</script>`,
        '<div title="<section id=&quot;slide-3&quot;>">Attribute value</div>',
        '<template><section id="slide-4">Never cloned</section></template>',
      ].join(""),
    );
    expect(shape.sectionCount).toBe(0);
    expect(shape.verdict).toBe("no-sections");
  });

  it("counts markup that is written in a different case", () => {
    // HTML tag and attribute names are case-insensitive, and a substring search for the
    // lowercase form would report this correct deck as having no sections at all.
    const shape = readArtifactSlideShape('<SECTION ID="slide-1">Uppercase</SECTION>');
    expect(shape.ids).toEqual(["slide-1"]);
    expect(shape.verdict).toBe("matches");
  });

  it("reads an entity in an id the way the renderer does", () => {
    const shape = readArtifactSlideShape('<section id="slide&#45;1">Entity</section>');
    expect(shape.ids).toEqual(["slide-1"]);
    expect(shape.verdict).toBe("matches");
  });
});

describe("the mode is the caller's business", () => {
  // The function takes no output mode: the mode is a property of the request, not of the
  // document, and a page-mode artifact is allowed to carry section landmarks. This test
  // pins the consequence — the check will happily describe a page against a contract that
  // never applied — so that the gate lives at the call site and is not forgotten:
  //
  //   const notice = outputMode === "slides" ? artifactSlideNotice(readArtifactSlideShape(html)) : "";
  it("describes page markup that merely contains a section, so the caller must gate on the mode", () => {
    const shape = readArtifactSlideShape(
      '<main><section id="hero"><h1>Every truck, one board</h1></section>' +
        '<section id="pricing"><h2>Plans</h2></section></main>',
    );
    expect(shape.sectionCount).toBe(2);
    expect(shape.verdict).toBe("off-sequence");
    expect(artifactSlideNotice(shape)).not.toBe("");
  });
});
