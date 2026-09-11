// @vitest-environment happy-dom

import { Window as HappyWindow } from "happy-dom";
import { afterEach, describe, expect, it } from "vitest";
import {
  ARTIFACT_STRUCTURE_SELECTOR,
  MAX_ARTIFACT_STRUCTURE_ENTRIES,
  clearCachedArtifactSections,
  getCachedArtifactSections,
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

function mockRect(element: Element, top: number): void {
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () => ({ ...RECT_100, bottom: top + 100, top, y: top }),
  });
}

function runAssembledStructure(html: string): readonly ArtifactSection[] {
  const { window, doc, script, post } = openMeasurement(html);
  const elements = (
    doc as unknown as { querySelectorAll: (selector: string) => Element[] }
  ).querySelectorAll(ARTIFACT_STRUCTURE_SELECTOR);
  elements.forEach((element, index) => mockRect(element, index * 120));
  window.eval(script);
  const result = readArtifactRenderCriticResult(post());
  if (result === null) throw new Error("Measurement script did not post a valid result");
  return result.structure ?? [];
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
    expect(byAnchor.get("site-header")?.rect).toMatchObject({ x: 0, width: 1280, height: 100 });
    expect(byAnchor.get("content")).toMatchObject({ tag: "main", depth: 1 });
    // The section's readable name is its heading text (text wins over aria-label).
    const pricing = [...byAnchor.values()].find((section) => section.tag === "section");
    expect(pricing).toMatchObject({ tag: "section", name: "Pricing", depth: 2 });
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
    mockRect(shown, 0);
    // The hidden h1 and the main (zero rect in happy-dom) keep their real boxes.
    window.eval(script);
    const result = readArtifactRenderCriticResult(post());
    if (result === null) throw new Error("Measurement script did not post a valid result");
    const tags = (result.structure ?? []).map((section) => section.tag);
    expect(tags).toContain("h2");
    expect(tags).not.toContain("h1");
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

  it("rejects unknown tags, bad anchors, depths, rects, and duplicates", () => {
    expect(readArtifactStructure([{ ...good, tag: "div" }])).toBeNull();
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
      structure: [{ anchor: "x", tag: "div", name: "Bad", depth: 0, rect: {} }],
    });
    expect(result).not.toBeNull();
    expect(result?.structure).toEqual([]);
  });
});
