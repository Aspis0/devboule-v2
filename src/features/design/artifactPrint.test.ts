// @vitest-environment happy-dom

/*
 * The print document is built with `DOMParser` and the artifact shape is read
 * with the same parser, so this needs a document environment — the header above
 * and `happy-dom` are what the other artifact tests use. Nothing here mounts a
 * component: every case is a fragment in and a document, a stylesheet rule or a
 * validated report out.
 */

import { describe, expect, it, vi } from "vitest";
import { ARTIFACT_CSP } from "./artifactCsp";
import { readArtifactSlideShape } from "./artifactSlides";
import {
  ARTIFACT_PRINT_CSP,
  ARTIFACT_PRINT_MESSAGE_KIND,
  ARTIFACT_PRINT_SANDBOX,
  ARTIFACT_PRINT_SOURCE,
  ArtifactPrintCspError,
  buildArtifactPrintCss,
  buildArtifactPrintDocument,
  deriveArtifactPrintCsp,
  readArtifactPrintLayout,
  readArtifactPrintReport,
} from "./artifactPrint";

const PAGE_FRAGMENT = "<main><h1>Our menu</h1><p>Body copy.</p></main>";

function deckFragment(ids: readonly string[]): string {
  return ids
    .map((id, index) => `<section id="${id}"><h2>Slide ${index + 1}</h2></section>`)
    .join("");
}

function layoutFor(fragment: string, outputMode?: "page" | "slides") {
  return readArtifactPrintLayout(readArtifactSlideShape(fragment), outputMode);
}

function parse(html: string): Document {
  return new DOMParser().parseFromString(html, "text/html");
}

function policyMetaContents(doc: Document): string[] {
  return [...doc.querySelectorAll("meta[http-equiv]")]
    .filter((meta) => meta.getAttribute("http-equiv")?.toLowerCase() === "content-security-policy")
    .map((meta) => meta.getAttribute("content") ?? "");
}

function printStyle(doc: Document): string {
  return [...doc.querySelectorAll("style")].map((style) => style.textContent ?? "").join("\n");
}

/** A URL that leaves the document, in any spelling the markup could carry. */
function isOffDocument(value: string): boolean {
  return /^(?:https?:)?\/\/|^blob:|^file:/i.test(value.trim());
}

function offDocumentReferences(doc: Document): string[] {
  const found: string[] = [];
  for (const element of doc.querySelectorAll("*")) {
    for (const attribute of ["src", "srcset", "poster", "data"]) {
      const value = element.getAttribute(attribute);
      if (value !== null && isOffDocument(value)) {
        found.push(`${element.tagName.toLowerCase()}[${attribute}]`);
      }
    }
  }
  for (const element of doc.querySelectorAll("link, use, image, script")) {
    const href = element.getAttribute("href");
    if (href !== null && isOffDocument(href)) {
      found.push(`${element.tagName.toLowerCase()}[href]`);
    }
  }
  if (/@import|url\(\s*['"]?(?:https?:)?\/\//i.test(printStyle(doc))) {
    found.push("<style>");
  }
  return found;
}

describe("the print policy", () => {
  it("derives it from the canvas one by swapping exactly the script directive", () => {
    // The same argument the render critic's derivation carries: the print
    // document must be the canvas document under one changed directive, and a
    // `.replace` that stops matching returns the canvas policy unchanged —
    // silently banning the print trigger instead of failing to build.
    expect(ARTIFACT_PRINT_CSP).not.toBe(ARTIFACT_CSP);
    expect(ARTIFACT_PRINT_CSP).toContain("script-src 'unsafe-inline'");
    expect(ARTIFACT_PRINT_CSP).not.toContain("script-src 'none'");
    expect(ARTIFACT_PRINT_CSP.replace("script-src 'unsafe-inline'", "script-src 'none'")).toBe(
      ARTIFACT_CSP,
    );
  });

  it("refuses a base policy that does not carry the script directive exactly once", () => {
    expect(() => deriveArtifactPrintCsp("script-src 'unsafe-inline'")).toThrow(
      ArtifactPrintCspError,
    );
    expect(() => deriveArtifactPrintCsp("script-src 'unsafe-inline'")).toThrow(/occurs 0 times/);
    expect(() => deriveArtifactPrintCsp("script-src 'none'; script-src 'none'")).toThrow(
      /occurs 2 times/,
    );
  });

  it("carries script and modal permission, and nothing else", () => {
    // Both are needed: the trigger is a script, and `window.print()` is gated by
    // the sandboxed modals flag. `allow-same-origin` must never be here — with
    // `allow-scripts` it is the documented sandbox escape.
    expect(ARTIFACT_PRINT_SANDBOX).toBe("allow-scripts allow-modals");
    expect(ARTIFACT_PRINT_SANDBOX).not.toContain("allow-same-origin");
    expect(ARTIFACT_PRINT_SANDBOX).not.toContain("allow-popups");
    expect(ARTIFACT_PRINT_SANDBOX).not.toContain("allow-top-navigation");
  });
});

describe("the deck/page decision", () => {
  it("prints a recorded slides run as a deck, however badly its ids are spelled", () => {
    // The mode is a fact about the artifact, so it wins outright: a recorded deck
    // whose ids are a mess is still a deck, and the naming is reported by
    // `artifactSlideNotice`, not here.
    for (const fragment of [
      deckFragment(["slide-1", "slide-2"]),
      deckFragment(["cover", "agenda"]),
      deckFragment(["slide-1", "slide-1"]),
      PAGE_FRAGMENT,
    ]) {
      expect(layoutFor(fragment, "slides")).toEqual({
        deck: true,
        orientation: "landscape",
        pageSize: "A4",
      });
    }
  });

  it("prints a recorded page run continuously, even when it is full of sections", () => {
    // THE REGRESSION, measured live 2026-09-11. A pricing page — three plan
    // cards, a FAQ, a footer, one continuous document — is three `<section>`
    // elements with no ids, and `sectionCount > 0` read it as a three-sheet
    // landscape deck. The decision is not allowed to look at the sections once
    // the producing run has said what it produced.
    const pricingPage = [
      '<section class="plans"><h2>Plans</h2></section>',
      '<section class="faq"><h2>FAQ</h2></section>',
      '<section class="footer"><p>Contact</p></section>',
    ].join("");
    expect(readArtifactSlideShape(pricingPage).sectionCount).toBe(3);
    expect(layoutFor(pricingPage, "page")).toEqual({
      deck: false,
      orientation: "portrait",
      pageSize: "A4",
    });
    // And the stylesheet that follows from it carries no break rule at all.
    const css = buildArtifactPrintCss(readArtifactSlideShape(pricingPage), "page");
    expect(css).toContain("size: A4 portrait");
    expect(css).not.toContain("break-before");
  });

  it("falls back to the shape when the run recorded no mode, and the shape is a deck", () => {
    // Absent is the artifact reopened from design history. The only evidence left
    // is the markup, and a section carrying a `slide-N` id is what shows the
    // document attempted the contract.
    expect(layoutFor(deckFragment(["slide-1", "slide-2", "slide-3"]))).toEqual({
      deck: true,
      orientation: "landscape",
      pageSize: "A4",
    });
    // One numbered section among unnamed ones is still an attempt.
    expect(layoutFor(deckFragment(["slide-1", "", ""]))).toEqual({
      deck: true,
      orientation: "landscape",
      pageSize: "A4",
    });
    // Numbered but out of order, and duplicated: naming problems, not a page.
    expect(layoutFor(deckFragment(["slide-2", "slide-1"])).deck).toBe(true);
    expect(layoutFor(deckFragment(["slide-1", "slide-1"])).deck).toBe(true);
  });

  it("falls back to the shape when the run recorded no mode, and unnamed sections are a page", () => {
    // The measured document itself, with no mode recorded: three sections, no
    // ids. It is a page, and so is one section with no id at all.
    expect(layoutFor('<section class="plans"><h2>Plans</h2></section>')).toEqual({
      deck: false,
      orientation: "portrait",
      pageSize: "A4",
    });
    expect(layoutFor("<section></section><section></section>").deck).toBe(false);
    // Ids that are not the contract's are not an attempt at it: `pricing` and
    // `faq` name landmarks of a page, and a document with no sections is a page.
    expect(layoutFor(deckFragment(["pricing", "faq", "footer"])).deck).toBe(false);
    expect(layoutFor(deckFragment(["cover", "agenda"])).deck).toBe(false);
    expect(layoutFor(PAGE_FRAGMENT)).toEqual({
      deck: false,
      orientation: "portrait",
      pageSize: "A4",
    });
  });
});

describe("the print stylesheet", () => {
  it("gives a deck one slide per page, in landscape, with no paper margin", () => {
    const css = buildArtifactPrintCss(
      readArtifactSlideShape(deckFragment(["slide-1", "slide-2"])),
      "slides",
    );

    expect(css).toContain("size: A4 landscape");
    expect(css).toContain("margin: 0");
    expect(css).toContain("break-before: page");
    expect(css).toContain("break-inside: avoid");
    // The first slide is already at the top of page one; exempting it is what
    // keeps a forced break at the very start of the document from becoming a
    // blank leading page.
    expect(css).toContain("section:first-of-type");
    expect(css).toContain("break-before: auto");
  });

  it("lets a page-mode artifact flow continuously, in portrait", () => {
    const css = buildArtifactPrintCss(readArtifactSlideShape(PAGE_FRAGMENT));

    expect(css).toContain("size: A4 portrait");
    expect(css).toContain("margin: 0");
    expect(css).not.toContain("break-before");
  });

  it("keeps backgrounds and stops orphan headings in both modes", () => {
    for (const [fragment, mode] of [
      [PAGE_FRAGMENT, "page"],
      [deckFragment(["slide-1"]), "slides"],
    ] as const) {
      const css = buildArtifactPrintCss(readArtifactSlideShape(fragment), mode);
      // Exact colour, or the panels and bands most artifacts are made of are
      // dropped to save ink.
      expect(css).toContain("print-color-adjust: exact");
      // A heading must not be the last thing on a page.
      expect(css).toContain("break-after: avoid");
    }
  });
});

describe("the print document", () => {
  it("is the standalone export under the print policy, not the canvas one", () => {
    const documentHtml = buildArtifactPrintDocument(PAGE_FRAGMENT, "Run title");
    const doc = parse(documentHtml);

    expect(documentHtml.startsWith("<!DOCTYPE html>")).toBe(true);
    expect(policyMetaContents(doc)).toEqual([ARTIFACT_PRINT_CSP]);
    expect(documentHtml).not.toContain(ARTIFACT_CSP);
    // The export's own furniture survives, so the printed page is the document
    // the user can also save and copy.
    expect(documentHtml).toContain('<meta charset="utf-8">');
    expect(documentHtml).toContain("<title>Our menu</title>");
    expect(doc.body.querySelector("main")?.textContent).toContain("Our menu");
  });

  it("carries one inline trigger and refuses to fetch anything", () => {
    const documentHtml = buildArtifactPrintDocument(PAGE_FRAGMENT, "Run title");
    const doc = parse(documentHtml);
    const scripts = [...doc.querySelectorAll("script")];

    expect(scripts).toHaveLength(1);
    expect(scripts[0]?.getAttribute("src")).toBeNull();
    expect(scripts[0]?.textContent).toContain("window.print()");
    expect(scripts[0]?.textContent).toContain(ARTIFACT_PRINT_SOURCE);
    expect(scripts[0]?.textContent).toContain(ARTIFACT_PRINT_MESSAGE_KIND);
    expect(doc.querySelectorAll("link")).toHaveLength(0);
    // The stylesheet this module adds is inline and pulls nothing of its own:
    // no imported sheet, no web font, no background image.
    expect(printStyle(doc)).not.toContain("@import");
    expect(printStyle(doc)).not.toContain("url(");
    expect(offDocumentReferences(doc)).toEqual([]);
    // And the only policy in the document is the canvas one with the script
    // directive swapped, so every fetch the canvas bans stays banned here.
    expect(policyMetaContents(doc)).toEqual([ARTIFACT_PRINT_CSP]);
  });

  it("prints the artifact's own stylesheet and strips its scripts", () => {
    // The policy this document runs under allows inline script, so the
    // artifact's scripts have to go before insertion: they never ran under the
    // canvas policy either, and running them for the first time at print would
    // be an execution the user never previewed.
    const documentHtml = buildArtifactPrintDocument(
      '<style>.card { color: red; }</style><main onclick="steal()">Card</main><script>steal()</script>',
      "T",
    );
    const doc = parse(documentHtml);

    expect(printStyle(doc)).toContain(".card { color: red; }");
    expect(doc.querySelector("main")?.hasAttribute("onclick")).toBe(false);
    expect(documentHtml).not.toContain("steal()</script>");
    expect([...doc.querySelectorAll("script")]).toHaveLength(1);
  });

  it("takes the shape from the document it prints, and the mode from the run", () => {
    const documentHtml = buildArtifactPrintDocument(
      deckFragment(["slide-1", "slide-2"]),
      "T",
      "slides",
    );
    const doc = parse(documentHtml);

    expect(printStyle(doc)).toContain("size: A4 landscape");
    expect(printStyle(doc)).toContain("break-before: page");

    // Same markup, recorded as a page: continuous portrait, no section rules.
    const asPage = parse(
      buildArtifactPrintDocument(deckFragment(["slide-1", "slide-2"]), "T", "page"),
    );
    expect(printStyle(asPage)).toContain("size: A4 portrait");
    expect(printStyle(asPage)).not.toContain("break-before");
  });

  it("prints a page-mode artifact instead of refusing it", () => {
    const documentHtml = buildArtifactPrintDocument(PAGE_FRAGMENT, "T");
    const doc = parse(documentHtml);

    expect(printStyle(doc)).toContain("size: A4 portrait");
    expect(doc.body.querySelector("main")?.textContent).toContain("Body copy.");
  });

  it("fails loudly when the export stops carrying exactly one policy meta", async () => {
    // A guard, not a decoration: appending a second policy instead of replacing
    // the exported one would be inert (CSP is additive, most restrictive wins)
    // and the print trigger would never run. So the derivation of the document
    // is exercised with an export that has no canvas policy at all.
    vi.resetModules();
    vi.doMock("./artifactExport", () => ({
      buildStandaloneArtifactHtml: () =>
        '<!DOCTYPE html>\n<html lang="en"><head><title>T</title></head><body><main>Hi</main></body></html>\n',
    }));
    try {
      const fresh = await import("./artifactPrint");
      expect(() => fresh.buildArtifactPrintDocument("<main>Hi</main>", "T")).toThrow(
        fresh.ArtifactPrintCspError,
      );
    } finally {
      vi.doUnmock("./artifactExport");
      vi.resetModules();
    }
  });
});

describe("the print report reader", () => {
  it("accepts the two statuses the frame can send, with a bounded failure detail", () => {
    expect(
      readArtifactPrintReport({
        kind: ARTIFACT_PRINT_MESSAGE_KIND,
        source: ARTIFACT_PRINT_SOURCE,
        version: 1,
        status: "printed",
      }),
    ).toEqual({
      kind: ARTIFACT_PRINT_MESSAGE_KIND,
      source: ARTIFACT_PRINT_SOURCE,
      version: 1,
      status: "printed",
    });
    expect(
      readArtifactPrintReport({
        kind: ARTIFACT_PRINT_MESSAGE_KIND,
        message: "The print dialog could not be opened.",
        source: ARTIFACT_PRINT_SOURCE,
        version: 1,
        status: "failed",
      }),
    ).toMatchObject({ message: "The print dialog could not be opened.", status: "failed" });
  });

  it("drops anything that is not a bounded report from this feature", () => {
    const valid = {
      kind: ARTIFACT_PRINT_MESSAGE_KIND,
      source: ARTIFACT_PRINT_SOURCE,
      version: 1,
      status: "printed",
    };
    expect(readArtifactPrintReport(null)).toBeNull();
    expect(readArtifactPrintReport("printed")).toBeNull();
    expect(readArtifactPrintReport({ ...valid, kind: "something-else" })).toBeNull();
    expect(readArtifactPrintReport({ ...valid, source: "somewhere-else" })).toBeNull();
    expect(readArtifactPrintReport({ ...valid, version: 2 })).toBeNull();
    expect(readArtifactPrintReport({ ...valid, status: "cancelled" })).toBeNull();
    expect(readArtifactPrintReport({ ...valid, message: "" })).toBeNull();
    expect(readArtifactPrintReport({ ...valid, message: "x".repeat(161) })).toBeNull();
  });
});
