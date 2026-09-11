// @vitest-environment happy-dom

import { Window as HappyWindow } from "happy-dom";
import { afterEach, describe, expect, it } from "vitest";
import {
  ARTIFACT_STRUCTURE_TAGS,
  MAX_ARTIFACT_STRUCTURE_ENTRIES,
  clearCachedArtifactSections,
  getCachedArtifactSections,
  getCachedArtifactStructure,
  readArtifactStructure,
  sectionsToLayers,
  setCachedArtifactSections,
  type ArtifactSection,
} from "./artifactStructure";
import {
  ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
  ARTIFACT_RENDER_CRITIC_SOURCE,
  buildArtifactMeasurementSrcDoc,
  readArtifactRenderCriticResult,
} from "./artifactRenderCritic";

const RECT_100 = {
  bottom: 100,
  height: 100,
  left: 0,
  right: 1280,
  top: 0,
  width: 1280,
  x: 0,
  y: 0,
};

function openMeasurement(html: string): {
  window: HappyWindow;
  doc: Document;
  script: string;
  post: () => unknown;
} {
  const srcDoc = buildArtifactMeasurementSrcDoc(html);
  const scriptMatch = srcDoc.match(/<script>([\s\S]*?)<\/script>/);
  if (scriptMatch === null) throw new Error("Measurement script was not assembled");
  const measurementWindow = new HappyWindow({ url: "http://measurement.test/" });
  let posted: unknown;
  const parent = { postMessage: (message: unknown) => (posted = message) };
  Object.defineProperty(measurementWindow, "parent", {
    configurable: true,
    value: parent,
  });
  measurementWindow.document.write(srcDoc.replace(scriptMatch[0], ""));
  return {
    window: measurementWindow,
    doc: measurementWindow.document as unknown as Document,
    script: scriptMatch[1] ?? "",
    post: () => {
      measurementWindow.happyDOM.close();
      return posted;
    },
  };
}

type MockBox = {
  bottom: number;
  height: number;
  left: number;
  right: number;
  top: number;
  width: number;
  x: number;
  y: number;
};

function mockBox(element: Element, box: MockBox): void {
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () => box,
  });
}

/**
 * Runs the assembled measurement script against a fragment. Every element gets
 * a non-empty box, because the collector's first filter is a real one: the
 * layout here is a stub, but the structural decision it makes is not.
 */
function runAssembledStructure(
  html: string,
  boxFor?: (element: Element, index: number) => MockBox,
): readonly ArtifactSection[] {
  const { window, doc, script, post } = openMeasurement(html);
  const elements = (
    doc as unknown as { querySelectorAll: (selector: string) => Element[] }
  ).querySelectorAll("*");
  elements.forEach((element, index) => {
    const top = index * 120;
    mockBox(
      element,
      boxFor === undefined
        ? { ...RECT_100, bottom: top + 100, top, y: top }
        : boxFor(element, index),
    );
  });
  window.eval(script);
  const result = readArtifactRenderCriticResult(post());
  if (result === null) throw new Error("Measurement script did not post a valid result");
  return result.structure ?? [];
}

function indexOfAnchor(sections: readonly ArtifactSection[], anchor: string): number {
  const index = sections.findIndex((section) => section.anchor === anchor);
  if (index < 0) throw new Error(`Anchor not collected: ${anchor}`);
  return index;
}

/** The chain of ancestors the panel rebuilds from `parent`, innermost first. */
function ancestorChain(sections: readonly ArtifactSection[], index: number): string[] {
  const chain: string[] = [];
  const visited = new Set<number>();
  let current: number | null = index;
  while (current !== null) {
    if (visited.has(current)) throw new Error("Parent chain cycles");
    visited.add(current);
    const section: ArtifactSection | undefined = sections[current];
    if (section === undefined) throw new Error(`Dangling parent index: ${current}`);
    chain.push(section.anchor);
    current = section.parent ?? null;
  }
  return chain;
}

afterEach(() => {
  document.body.replaceChildren();
  clearCachedArtifactSections();
});

describe("artifact structure collector (frame pass)", () => {
  it("collects landmarks and headings with id anchors, names, depths, and rects", () => {
    const sections = runAssembledStructure(
      '<header id="site-header"><h1>Welcome home</h1></header>' +
        '<main id="content"><section aria-label="Pricing plans"><h2>Pricing</h2></section></main>',
    );

    const byAnchor = new Map(sections.map((section) => [section.anchor, section]));
    expect(byAnchor.get("site-header")).toMatchObject({
      tag: "header",
      name: "Welcome home",
      depth: 1,
    });
    expect(byAnchor.get("site-header")?.parent ?? null).toBeNull();
    expect(byAnchor.get("site-header")?.rect).toMatchObject({ x: 0, width: 1280, height: 100 });
    expect(byAnchor.get("content")).toMatchObject({ tag: "main", depth: 1 });
    expect(byAnchor.get("content")?.parent ?? null).toBeNull();
    // The section's readable name is its heading text (text wins over aria-label).
    const pricing = [...byAnchor.values()].find((section) => section.tag === "section");
    expect(pricing).toMatchObject({ tag: "section", name: "Pricing", depth: 2 });
    expect(pricing?.parent).toBe(indexOfAnchor(sections, "content"));
  });

  it("keeps every landmark and heading the old selector collected", () => {
    const sections = runAssembledStructure(
      "<header>H</header><nav>N</nav><main>M</main><aside>A</aside><footer>F</footer>" +
        "<section>S</section><article>R</article>" +
        "<h1>1</h1><h2>2</h2><h3>3</h3><h4>4</h4><h5>5</h5><h6>6</h6>",
    );
    expect(sections.map((section) => section.tag)).toEqual([...ARTIFACT_STRUCTURE_TAGS]);
  });

  it("computes a tag path anchor when the element has no id", () => {
    const sections = runAssembledStructure("<main><section><h2>Plain</h2></section></main>");
    const byTag = new Map(sections.map((section) => [section.tag, section]));
    expect(byTag.get("main")?.anchor).toBe("body[1]/main[1]");
    expect(byTag.get("section")?.anchor).toBe("body[1]/main[1]/section[1]");
    expect(byTag.get("h2")?.anchor).toBe("body[1]/main[1]/section[1]/h2[1]");
  });

  it("names an untitled landmark by role instead of its concatenated text", () => {
    const sections = runAssembledStructure(
      '<header><nav><a href="#docs">Docs</a><a href="#cli">CLI reference</a></nav></header>' +
        '<footer><a href="#log">Changelog</a> <a href="#src">Source</a></footer>',
    );
    const byTag = new Map(sections.map((section) => [section.tag, section]));
    expect(byTag.get("header")?.name).toBe("Header");
    expect(byTag.get("nav")?.name).toBe("Navigation");
    expect(byTag.get("footer")?.name).toBe("Footer");
  });

  it("names a landmark by its first heading, not its full text", () => {
    const sections = runAssembledStructure(
      "<main><section><h2>Pricing</h2><p>Plans for everyone and everything</p></section></main>",
    );
    const byTag = new Map(sections.map((section) => [section.tag, section]));
    expect(byTag.get("section")?.name).toBe("Pricing");
    expect(byTag.get("main")?.name).toBe("Pricing");
  });

  it("indexes same-tag siblings so two sections anchor differently", () => {
    const sections = runAssembledStructure("<main><section>A</section><section>B</section></main>");
    const anchors = sections
      .filter((section) => section.tag === "section")
      .map((section) => section.anchor);
    expect(anchors).toEqual(["body[1]/main[1]/section[1]", "body[1]/main[1]/section[2]"]);
  });

  it("names by heading text first, then aria-label, then id", () => {
    const sections = runAssembledStructure(
      '<main id="content">' +
        '<section aria-label="Label loses to text"><h2>Text wins</h2></section>' +
        '<nav aria-label="Primary"></nav>' +
        '<aside id="side-note"><span>   </span></aside>' +
        "</main>",
    );
    const byTag = new Map(sections.map((section) => [section.tag, section]));
    expect(byTag.get("h2")?.name).toBe("Text wins");
    expect(byTag.get("nav")?.name).toBe("Primary");
    const byAnchor = new Map(sections.map((section) => [section.anchor, section]));
    expect(byAnchor.get("side-note")?.name).toBe("side-note");
  });

  it("skips hidden and zero-area elements", () => {
    const { window, doc, script, post } = openMeasurement(
      '<main><h1 style="display: none">Hidden</h1><h2>Shown</h2></main>',
    );
    const query = (doc as unknown as { querySelector: (s: string) => Element | null })
      .querySelector;
    const shown = query.call(doc, "h2");
    if (shown === null) throw new Error("h2 missing");
    mockBox(shown, RECT_100);
    // The hidden h1 and the main (zero rect in happy-dom) keep their real boxes.
    window.eval(script);
    const result = readArtifactRenderCriticResult(post());
    if (result === null) throw new Error("Measurement script did not post a valid result");
    const tags = (result.structure ?? []).map((section) => section.tag);
    expect(tags).toContain("h2");
    expect(tags).not.toContain("h1");
  });

  // The complaint this rule exists for: the old selector walked landmarks and
  // headings only, so an eyebrow div and a slide counter span were unreachable.
  it("collects anonymous text leaves inside a section", () => {
    const sections = runAssembledStructure(
      '<section id="slide-1"><div>Introduction</div><span>01 — FleetView</span></section>',
    );
    const byAnchor = new Map(sections.map((section) => [section.anchor, section]));
    expect(byAnchor.get("body[1]/section[1]/div[1]")).toMatchObject({
      tag: "div",
      name: "Introduction",
      depth: 2,
    });
    expect(byAnchor.get("body[1]/section[1]/span[1]")).toMatchObject({
      tag: "span",
      name: "01 — FleetView",
      depth: 2,
    });
  });

  it("does not collect a container as a text leaf", () => {
    const sections = runAssembledStructure(
      '<section id="slide-1"><div class="wrap"><span>Only piece</span></div></section>',
    );
    expect(sections.map((section) => section.tag)).not.toContain("div");
    expect(sections.map((section) => section.tag)).toContain("span");
  });

  it("skips whitespace-only text leaves", () => {
    const sections = runAssembledStructure(
      '<section id="slide-1"><span class="spacer">   </span><span>Real</span></section>',
    );
    expect(
      sections.filter((section) => section.tag === "span").map((section) => section.name),
    ).toEqual(["Real"]);
  });

  it("collects controls and media the old selector could not reach", () => {
    const sections = runAssembledStructure(
      '<section id="hero">' +
        '<a href="#pricing">Pricing</a>' +
        '<button aria-label="Close dialog"></button>' +
        '<div role="button" aria-label="Play sample"></div>' +
        '<div tabindex="0" aria-label="Card"></div>' +
        '<span role="link" aria-label="Details"></span>' +
        "</section>",
    );
    const names = sections.map((section) => section.name);
    expect(names).toContain("Pricing");
    expect(names).toContain("Close dialog");
    expect(names).toContain("Play sample");
    expect(names).toContain("Card");
    expect(names).toContain("Details");
  });

  it("names media from their nearest authored description", () => {
    const sections = runAssembledStructure(
      "<main>" +
        '<figure id="fig"><img src="/img/hero-chart.png" alt="Revenue by quarter">' +
        "<figcaption>Q3 revenue</figcaption></figure>" +
        '<svg aria-label="Logo mark"><title>Fallback title</title><rect /></svg>' +
        '<img id="deco" src="/img/decoration.png">' +
        '<img src="data:image/gif;base64,AAAA">' +
        '<picture><source srcset="/img/team.webp"><img src="/img/team.jpg" alt="The team"></picture>' +
        '<video src="/media/intro.mp4"></video>' +
        '<canvas id="chart"></canvas>' +
        "</main>",
    );
    const byAnchor = new Map(sections.map((section) => [section.anchor, section]));
    // aria-label outranks the svg's own <title>, exactly as the accessible name does.
    expect(byAnchor.get("body[1]/main[1]/svg[1]")?.name).toBe("Logo mark");
    expect(byAnchor.get("fig")?.name).toBe("Q3 revenue");
    expect(byAnchor.get("deco")?.name).toBe("decoration.png");
    const imageAlts = sections.filter((section) => section.tag === "img").map((s) => s.name);
    expect(imageAlts).toContain("Revenue by quarter");
    expect(imageAlts).toContain("The team");
    // A data: URL has no name worth showing; the role name is the honest fallback.
    expect(imageAlts).toContain("Image");
    expect(sections.find((section) => section.tag === "video")?.name).toBe("intro.mp4");
    expect(byAnchor.get("chart")?.name).toBe("chart");
  });

  it("skips page furniture even when a stray attribute would qualify it", () => {
    const sections = runAssembledStructure(
      '<style>.a{color:red}</style><p>Body text</p><div tabindex="0">Real</div>',
    );
    expect(sections.map((section) => section.tag)).not.toContain("style");
    expect(sections.map((section) => section.tag)).not.toContain("script");
    expect(sections.map((section) => section.name)).toEqual(["Body text", "Real"]);
  });
});

describe("artifact structure hierarchy", () => {
  it("records the nearest collected ancestor across a skipped wrapper", () => {
    const sections = runAssembledStructure(
      '<section id="slide-1"><div class="eyebrow"><span>01 — FleetView</span></div><h2>Title</h2></section>',
    );
    const slideIndex = indexOfAnchor(sections, "slide-1");
    const span = sections.find((section) => section.name === "01 — FleetView");
    expect(span?.parent).toBe(slideIndex);
    // The skipped div still counts for DOM depth: that gap is the difference
    // between "the piece" and "the section that holds it".
    expect(span?.depth).toBe(3);
    const heading = sections.find((section) => section.tag === "h2");
    expect(heading?.parent).toBe(slideIndex);
    expect(heading?.depth).toBe(2);
  });

  it("reconstructs the ancestor chain of a leaf up to its section", () => {
    const sections = runAssembledStructure(
      '<section id="slide-1"><h2>FleetView</h2><div class="eyebrow"><span>01 — FleetView</span></div></section>' +
        '<section id="slide-2"><h2>Pricing</h2><p>Plans</p></section>',
    );
    const spanIndex = sections.findIndex((section) => section.name === "01 — FleetView");
    expect(ancestorChain(sections, spanIndex)).toEqual([
      "body[1]/section[1]/div[1]/span[1]",
      "slide-1",
    ]);
    const paragraphIndex = sections.findIndex((section) => section.tag === "p");
    expect(ancestorChain(sections, paragraphIndex)).toEqual(["body[1]/section[2]/p[1]", "slide-2"]);
    // Every entry resolves, and no chain dangles or cycles.
    sections.forEach((_, index) => {
      expect(ancestorChain(sections, index).length).toBeGreaterThan(0);
    });
  });

  it("derives the first level from parent == null, even through uncollected wrappers", () => {
    const sections = runAssembledStructure(
      '<div class="deck"><section id="slide-1"><h2>A</h2></section>' +
        '<section id="slide-2"><h2>B</h2></section></div>',
    );
    const roots = sections.filter((section) => (section.parent ?? null) === null);
    expect(roots.map((section) => section.anchor)).toEqual(["slide-1", "slide-2"]);
  });

  it("derives the first level below a landmark without guessing", () => {
    const sections = runAssembledStructure(
      '<main id="page"><div class="deck"><section id="slide-1"><h2>A</h2></section>' +
        '<section id="slide-2"><h2>B</h2></section></div></main>',
    );
    expect(
      sections
        .filter((section) => (section.parent ?? null) === null)
        .map((section) => section.anchor),
    ).toEqual(["page"]);
    const pageIndex = indexOfAnchor(sections, "page");
    expect(
      sections.filter((section) => section.parent === pageIndex).map((section) => section.anchor),
    ).toEqual(["slide-1", "slide-2"]);
  });

  it("derives parent, first child, and previous/next sibling from the flat list", () => {
    const sections = runAssembledStructure(
      '<section id="slide"><h2>Head</h2><p>One</p><p>Two</p></section>',
    );
    const slideIndex = indexOfAnchor(sections, "slide");
    const children = sections
      .map((section, index) => ({ index, section }))
      .filter((entry) => entry.section.parent === slideIndex);
    expect(children.map((entry) => entry.section.tag)).toEqual(["h2", "p", "p"]);
    // First child is the lowest-index entry whose parent is this entry; the
    // children are already in document order, so siblings are adjacent moves.
    expect(children[0]?.index).toBe(slideIndex + 1);
    expect(children[1]?.index).toBe((children[0]?.index ?? 0) + 1);
    expect(children[2]?.index).toBe((children[1]?.index ?? 0) + 1);
    const previousSibling = children[2] === undefined ? undefined : children[1];
    expect(previousSibling?.section.name).toBe("One");
    // A root entry has no parent and no sibling above it.
    const root = sections[0];
    expect(root?.parent ?? null).toBeNull();
  });

  it("orders entries by document order so a hit test can break a tie", () => {
    const sections = runAssembledStructure(
      '<section id="slide-1"><h2>Title</h2><p>Body copy</p></section>',
    );
    expect(sections.map((section) => section.tag)).toEqual(["section", "h2", "p"]);
    sections.forEach((section, index) => {
      // Parents precede their descendants, which is what the validator enforces.
      expect(section.parent ?? -1).toBeLessThan(index);
    });
  });

  it("decides the innermost box from rect, depth, and document order alone", () => {
    const sections = runAssembledStructure(
      '<section id="slide-1"><div class="eyebrow"><span>01 — FleetView</span></div></section>',
      (element) => {
        const tag = element.tagName.toLowerCase();
        if (tag === "section") {
          return {
            bottom: 720,
            height: 720,
            left: 0,
            right: 1280,
            top: 0,
            width: 1280,
            x: 0,
            y: 0,
          };
        }
        if (tag === "div") {
          return { bottom: 60, height: 60, left: 64, right: 464, top: 0, width: 400, x: 64, y: 0 };
        }
        return { bottom: 24, height: 24, left: 64, right: 264, top: 0, width: 200, x: 64, y: 0 };
      },
    );
    // The rule the panel applies, verbatim: smallest containing area wins, then
    // the deepest box, then the last one in document order.
    const point = { x: 100, y: 10 };
    const hit = sections
      .map((section, index) => ({ index, section }))
      .filter(
        ({ section }) =>
          point.x >= section.rect.x &&
          point.x <= section.rect.x + section.rect.width &&
          point.y >= section.rect.y &&
          point.y <= section.rect.y + section.rect.height,
      )
      .sort((left, right) => {
        const leftArea = left.section.rect.width * left.section.rect.height;
        const rightArea = right.section.rect.width * right.section.rect.height;
        if (leftArea !== rightArea) return leftArea - rightArea;
        if (left.section.depth !== right.section.depth)
          return right.section.depth - left.section.depth;
        return right.index - left.index;
      })[0];
    expect(hit?.section.name).toBe("01 — FleetView");
    expect(hit?.section.parent).toBe(indexOfAnchor(sections, "slide-1"));
  });
});

describe("readArtifactStructure validation", () => {
  const good = {
    anchor: "site-header",
    tag: "header",
    name: "Welcome",
    depth: 1,
    rect: { x: 0, y: 0, width: 1280, height: 120 },
  };

  it("accepts a well-formed index", () => {
    expect(readArtifactStructure([good])).toEqual([good]);
    expect(readArtifactStructure([])).toEqual([]);
  });

  it("accepts the leaf, control, and media tags the structural rule adds", () => {
    for (const tag of ["div", "span", "p", "li", "td", "img", "svg", "canvas", "a", "button"]) {
      expect(readArtifactStructure([{ ...good, tag }])).toEqual([{ ...good, tag }]);
    }
  });

  it("rejects page furniture, bad anchors, depths, rects, and duplicates", () => {
    for (const tag of ["script", "style", "head", "html", "body", "DIV", "my widget"]) {
      expect(readArtifactStructure([{ ...good, tag }])).toBeNull();
    }
    expect(readArtifactStructure([{ ...good, anchor: "" }])).toBeNull();
    expect(readArtifactStructure([{ ...good, depth: -1 }])).toBeNull();
    expect(readArtifactStructure([{ ...good, name: "" }])).toBeNull();
    expect(
      readArtifactStructure([{ ...good, rect: { ...good.rect, width: Number.NaN } }]),
    ).toBeNull();
    expect(readArtifactStructure([good, good])).toBeNull();
    expect(readArtifactStructure("nope")).toBeNull();
    expect(
      readArtifactStructure(new Array(MAX_ARTIFACT_STRUCTURE_ENTRIES + 1).fill(good)),
    ).toBeNull();
  });

  it("accepts a parent that points backwards and rejects a dangling or forward one", () => {
    const leaf = {
      anchor: "body[1]/main[1]/p[1]",
      tag: "p",
      name: "Body copy",
      depth: 2,
      parent: 0,
      rect: { x: 0, y: 0, width: 100, height: 20 },
    };
    expect(readArtifactStructure([good, leaf])).toEqual([good, leaf]);
    // Root level: absent and null both mean "no collected ancestor".
    expect(readArtifactStructure([{ ...good, parent: null }])).toEqual([good]);
    expect(readArtifactStructure([{ ...leaf, parent: 1 }])).toBeNull();
    expect(readArtifactStructure([{ ...leaf, parent: 4 }])).toBeNull();
    expect(readArtifactStructure([{ ...leaf, parent: 0.5 }])).toBeNull();
    expect(readArtifactStructure([{ ...leaf, parent: "0" }])).toBeNull();
  });
});

describe("sectionsToLayers", () => {
  it("places section layers inside the artifact frame at origin plus page rect", () => {
    const sections: ArtifactSection[] = [
      {
        anchor: "content",
        tag: "main",
        name: "Content",
        depth: 1,
        rect: { x: 10, y: 20, width: 100, height: 50 },
      },
    ];
    expect(sectionsToLayers(sections, { x: 60, y: 46 })).toEqual([
      {
        id: "section:content",
        name: "Content",
        kind: "SECTION",
        transform: { x: 70, y: 66, width: 100, height: 50 },
        section: { tag: "main", anchor: "content" },
      },
    ]);
  });

  it("carries the measured parent index through as the parent layer id", () => {
    const sections: ArtifactSection[] = [
      {
        anchor: "slide-1",
        tag: "section",
        name: "Slide one",
        depth: 1,
        parent: null,
        rect: { x: 0, y: 0, width: 1280, height: 400 },
      },
      {
        anchor: "slide-1-title",
        tag: "h2",
        name: "Title",
        depth: 2,
        parent: 0,
        rect: { x: 40, y: 20, width: 600, height: 60 },
      },
    ];
    // A root carries no parentId; a child points at the layer id its parent
    // anchor produced, which is what the panel walks.
    expect(sectionsToLayers(sections, { x: 0, y: 0 }).map((layer) => layer.section)).toEqual([
      { tag: "section", anchor: "slide-1" },
      { tag: "h2", anchor: "slide-1-title", parentId: "section:slide-1" },
    ]);
  });

  it("reads a dangling parent index as a root instead of dropping the child", () => {
    const sections: ArtifactSection[] = [
      {
        anchor: "orphan",
        tag: "p",
        name: "Orphan",
        depth: 1,
        parent: 7,
        rect: { x: 0, y: 0, width: 100, height: 20 },
      },
    ];
    expect(sectionsToLayers(sections, { x: 0, y: 0 })[0]?.section).toEqual({
      tag: "p",
      anchor: "orphan",
    });
  });
});

describe("artifact structure cache", () => {
  it("returns what the critic stored and evicts the oldest entry", () => {
    const html = "<main>A</main>";
    expect(getCachedArtifactSections(html)).toBeUndefined();
    const sections: ArtifactSection[] = [
      {
        anchor: "a",
        tag: "main",
        name: "A",
        depth: 1,
        rect: { x: 0, y: 0, width: 1, height: 1 },
      },
    ];
    setCachedArtifactSections(html, sections);
    expect(getCachedArtifactSections(html)).toBe(sections);
    for (let index = 0; index < 8; index += 1) {
      setCachedArtifactSections(`<main>${index}</main>`, []);
    }
    expect(getCachedArtifactSections(html)).toBeUndefined();
  });

  it("keeps the measured page height alongside the sections", () => {
    const html = "<main>Tall</main>";
    setCachedArtifactSections(html, [], 3600);
    expect(getCachedArtifactStructure(html)).toEqual({ sections: [], contentHeight: 3600 });
    // An entry stored without a height reads as unmeasured, not as zero.
    setCachedArtifactSections("<main>Short</main>", []);
    expect(getCachedArtifactStructure("<main>Short</main>")).toEqual({ sections: [] });
  });
});

describe("critic result carries the structure", () => {
  it("validates the frame-posted index on the same message as the findings", () => {
    const sections = runAssembledStructure('<main id="m"><h1>T</h1></main>');
    expect(sections.map((section) => section.anchor)).toEqual(["m", "body[1]/main[1]/h1[1]"]);
  });

  it("keeps messages without a structure (old senders) valid with an empty index", () => {
    const result = readArtifactRenderCriticResult({
      kind: ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
      source: ARTIFACT_RENDER_CRITIC_SOURCE,
      version: 1,
      findings: [],
    });
    expect(result).not.toBeNull();
    expect(result?.structure).toEqual([]);
  });

  it("drops an invalid structure to empty without losing the findings", () => {
    const result = readArtifactRenderCriticResult({
      kind: ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
      source: ARTIFACT_RENDER_CRITIC_SOURCE,
      version: 1,
      findings: [],
      structure: [{ anchor: "x", tag: "script", name: "Bad", depth: 0, rect: {} }],
    });
    expect(result).not.toBeNull();
    expect(result?.structure).toEqual([]);
  });
});

// The cap exists for the postMessage, not for a list nobody draws any more. The
// fixtures below are the two shapes a generated page actually takes: a long
// marketing page, and a list-heavy reference page. Both are measured, not
// guessed, so the constant next to them can cite them.
const DENSE_LANDING_HTML = [
  '<header id="site-header"><a class="brand" href="/">FleetView</a>',
  '<nav aria-label="Primary">',
  ["Product", "Solutions", "Pricing", "Docs", "Blog", "Sign in"]
    .map((label, position) => `<a href="#nav-${position}">${label}</a>`)
    .join(""),
  '</nav><a class="cta" href="/start">Start free</a></header>',
  "<main>",
  '<section id="hero"><div class="eyebrow">Fleet telemetry for growing teams</div>',
  "<h1>Every truck, every route, one board</h1>",
  "<p>Live positions, maintenance windows and driver hours in one view your dispatchers already know.</p>",
  '<div class="actions"><a href="/demo">Book a demo</a><a href="/docs">Read the docs</a></div>',
  '<img src="/img/dashboard.png" alt="Dispatch board with live vehicle positions"></section>',
  '<section id="features"><h2>Built for the dispatcher shift</h2>',
  ...Array.from(
    { length: 12 },
    (_, card) =>
      `<article class="card"><h3>Feature ${card + 1}</h3>` +
      `<p>What feature ${card + 1} changes for the team on shift.</p>` +
      `<a href="#f${card + 1}">Read more</a></article>`,
  ),
  "</section>",
  '<section id="pricing"><h2>Plans</h2><table><caption>Monthly pricing</caption><thead><tr>',
  "<th>Plan</th><th>Seats</th><th>Vehicles</th><th>Support</th><th>Price</th></tr></thead><tbody>",
  ...Array.from(
    { length: 8 },
    (_, row) => `<tr><td>Row ${row + 1}</td><td>10</td><td>50</td><td>Email</td><td>$99</td></tr>`,
  ),
  "</tbody></table></section>",
  '<section id="faq"><h2>Questions</h2>',
  ...Array.from(
    { length: 8 },
    (_, item) =>
      `<details><summary>Question ${item + 1}</summary>` +
      `<p>The answer to question ${item + 1}, short and plain.</p></details>`,
  ),
  "</section>",
  '<section id="cta"><h2>Start the trial</h2><p>Fourteen days, no card.</p>',
  '<button type="button">Create account</button></section>',
  "</main>",
  '<footer id="site-footer"><nav aria-label="Footer">',
  ...Array.from({ length: 20 }, (_, link) => `<a href="#l${link}">Link ${link + 1}</a>`),
  "</nav><p>© 2026 FleetView</p></footer>",
].join("");

const DENSE_REFERENCE_HTML = [
  '<header id="docs-header"><h1>API reference</h1><a href="/">Home</a></header>',
  '<nav id="toc" aria-label="On this page">',
  ...Array.from({ length: 60 }, (_, link) => `<a href="#t${link}">Endpoint ${link + 1}</a>`),
  "</nav>",
  '<main id="docs">',
  ...Array.from(
    { length: 30 },
    (_, section) =>
      `<section id="doc-${section}"><h2>Endpoint ${section + 1}</h2>` +
      `<p>The endpoint ${section + 1} returns a paginated list.</p>` +
      `<p>Call it with a bearer token and a page cursor.</p></section>`,
  ),
  '<section id="fields"><h2>Field reference</h2><table><caption>Response fields</caption><thead><tr>',
  "<th>Field</th><th>Type</th><th>Notes</th></tr></thead><tbody>",
  ...Array.from(
    { length: 120 },
    (_, row) => `<tr><td>field_${row + 1}</td><td>string</td><td>Set on write.</td></tr>`,
  ),
  "</tbody></table></section></main>",
  '<footer id="docs-footer">',
  ...Array.from({ length: 20 }, (_, link) => `<a href="#f${link}">Footer link ${link + 1}</a>`),
  "</footer>",
].join("");

function payloadBytes(sections: readonly ArtifactSection[]): number {
  return JSON.stringify(sections).length;
}

describe("artifact structure cap and payload", () => {
  it("keeps a dense landing page whole inside the cap", () => {
    const sections = runAssembledStructure(DENSE_LANDING_HTML);
    expect(sections.length).toBeLessThanOrEqual(MAX_ARTIFACT_STRUCTURE_ENTRIES);
    // Measured 161 entries and 25.5 KiB of JSON. The bounds are ceilings with
    // headroom for fixture drift, not targets.
    expect(sections.length).toBeGreaterThan(150);
    expect(payloadBytes(sections)).toBeLessThan(64 * 1024);
  });

  it("keeps a list-heavy reference page inside the cap", () => {
    const sections = runAssembledStructure(DENSE_REFERENCE_HTML);
    expect(sections.length).toBeLessThanOrEqual(MAX_ARTIFACT_STRUCTURE_ENTRIES);
    // Measured 572 entries and 93.3 KiB of JSON: the shape the cap is sized for.
    expect(sections.length).toBeGreaterThan(300);
  });

  it("bounds the worst-case payload at the cap", () => {
    const worst = Array.from({ length: MAX_ARTIFACT_STRUCTURE_ENTRIES }, (_, index) => ({
      anchor: "a".repeat(220),
      tag: "div",
      name: "n".repeat(80),
      depth: 12,
      parent: index === 0 ? null : index - 1,
      rect: { x: 1234.56, y: 12345.67, width: 1280, height: 12345.67 },
    }));
    const bytes = payloadBytes(worst);
    // Measured 332 KiB when every anchor and name sits at its character budget:
    // the number the constant's comment cites, with a loose regression bound.
    expect(bytes).toBeGreaterThan(0);
    expect(bytes / 1024).toBeLessThan(384);
  });
});
