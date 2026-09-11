import type { DesignLayer } from "./designHost";

/**
 * Structural index of a generated page: landmarks, headings, anonymous text
 * leaves, interactive controls, and media measured in the render critic's
 * off-screen frame, in the same single pass as the render checks. The display
 * iframe is unreadable from the parent (`sandbox=""`, `script-src 'none'`,
 * `inert`), so this index is the only thing that feeds the Layers panel and the
 * section hit zones for a generated artifact.
 *
 * Membership is structural, not a tag list. The tags a person wants to click are
 * usually anonymous (an eyebrow `<div>`, a slide counter `<span>`), so widening
 * the selector to more text tags would still miss them. An element is collected
 * when it is at least one of:
 *
 * - a landmark or heading (`ARTIFACT_STRUCTURE_TAGS`);
 * - a text leaf: no element children and non-empty trimmed text;
 * - interactive: `a`, `button`, `input`, `select`, `textarea`, a `button`/`link`
 *   ARIA role, or a `tabindex` attribute;
 * - media (`ARTIFACT_STRUCTURE_MEDIA_TAGS`).
 *
 * Rejection rules, after membership is decided: page furniture (`body`,
 * `head`, `html`, `script`, `style`) and anything inside `head`; a box that is
 * missing, non-finite, or has no area; `display: none`; an anchor that comes
 * out empty (an id over the posted budget and no path the builder will
 * return) or that repeats an earlier entry's anchor; and
 * `MAX_ARTIFACT_STRUCTURE_ENTRIES`, past which the tail of the page is left
 * unindexed.
 *
 * The list is flat, in document order, and each entry carries the index of its
 * nearest collected ancestor. That single number is the tree: the panel walks
 * from a clicked phrase up to the section that holds it, reads the root level as
 * `parent == null`, and derives parent/child/sibling moves for keyboard
 * navigation without a second measurement.
 */

/**
 * Landmarks and headings: collected whichever other rule they also match.
 * Ordered as the old selector walked them.
 */
export const ARTIFACT_STRUCTURE_TAGS = [
  "header",
  "nav",
  "main",
  "aside",
  "footer",
  "section",
  "article",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
] as const;

/** Media collected by the structural rule. */
export const ARTIFACT_STRUCTURE_MEDIA_TAGS = [
  "img",
  "svg",
  "figure",
  "picture",
  "video",
  "canvas",
] as const;

/** Tags collected as interactive controls. */
export const ARTIFACT_STRUCTURE_INTERACTIVE_TAGS = [
  "a",
  "button",
  "input",
  "select",
  "textarea",
] as const;

/** ARIA roles that make an otherwise anonymous element a control. */
export const ARTIFACT_STRUCTURE_INTERACTIVE_ROLES = ["button", "link"] as const;

/**
 * Hard cap on collected elements per measurement.
 *
 * The panel never draws the whole index: a short navigator shows the root level
 * and everything else is discovered by clicking the canvas, so the cost of this
 * number is the `postMessage` clone and the retained array, not rendering. Two
 * measured shapes fix the number, both run through the assembled frame script
 * in `artifactStructure.test.ts`:
 *
 * - dense landing page (6-link nav, hero, 12 cards, 8x5 pricing table, 8 FAQ
 *   rows, 20-link footer): 161 entries, 25.5 KiB of JSON;
 * - list-heavy reference page (60-link sidebar, 30 documented sections, 120-row
 *   field table, 20-link footer): 572 entries, 93.3 KiB, the case this cap is
 *   sized for;
 * - theoretical ceiling, every anchor and name at its character budget: 332 KiB.
 *
 * 800 covers the reference page with ~40% headroom and bounds a realistic
 * clone at ~93 KiB, a one-time cost per artifact. Past the cap the tail of the
 * page is unindexed, exactly as before.
 */
export const MAX_ARTIFACT_STRUCTURE_ENTRIES = 800;

/** Longest readable name kept per element (heading text, aria-label, or id). */
export const MAX_ARTIFACT_STRUCTURE_NAME_CHARS = 80;

/** Validation-time bounds for one posted entry. Kept wide: a page may paint off-viewport. */
export const MAX_ARTIFACT_STRUCTURE_ANCHOR_CHARS = 220;
export const MAX_ARTIFACT_STRUCTURE_DEPTH = 64;
export const ARTIFACT_STRUCTURE_COORD_BOUND = 100_000;

/** One measured structural element, in page CSS px at the 1280 px page width. */
export interface ArtifactSection {
  /**
   * Stable identity across regenerations: the element's `id` when it has one,
   * otherwise a computed path (`body[1]/main[1]/section[2]/h2[1]`) of tag names
   * with 1-based same-tag sibling positions. A path anchor breaks when the page
   * structure moves; that break must surface as an orphaned note, never as a
   * silently dropped one. The element `id` is used only when it fits the
   * posted anchor budget; a longer one falls back to the path, because an
   * identity the protocol cannot carry is worse than a path.
   */
  anchor: string;
  /** Lowercased tag name of the collected element (`div`, `p`, `img`, ...). */
  tag: string;
  /**
   * Readable name, used for the hover label and the breadcrumb, never for a
   * list row: headings, text leaves, and controls keep their own text;
   * landmarks use their first contained heading; media use their nearest
   * authored description; then aria-label, then the id, then the role name.
   */
  name: string;
  /**
   * DOM nesting depth below `<body>`: a direct child of `<body>` is 1. It is
   * deeper than `parent`'s entry depth whenever a non-collected wrapper sits
   * between them, and that gap is what lets a hit test prefer the innermost
   * containing box.
   */
  depth: number;
  /**
   * Index, in this same list, of the nearest collected ancestor; absent or null
   * for a root-level entry. The tree is `child.parent === index`, the root level
   * is `parent == null`, and entries arrive in document order so same-parent
   * entries are already in sibling order. Rebuilt from this list alone, with no
   * second measurement.
   */
  parent?: number | null;
  rect: { x: number; y: number; width: number; height: number };
}

/**
 * Frame-side collector. Its source is interpolated into the critic's
 * measurement script (`collectArtifactStructure.toString()`), so it must stay
 * fully self-contained: no imports, no module-scope reads, no TS-only runtime
 * semantics (annotations are stripped by the bundler in prod and by esbuild in
 * tests, which is exactly why the sibling helpers in the critic survive the
 * same trip). Only DOM globals available inside the measurement frame.
 */
export function collectArtifactStructure(): ArtifactSection[] {
  const limit = 800;
  const nameLimit = 80;
  // Must stay equal to MAX_ARTIFACT_STRUCTURE_ANCHOR_CHARS: an entry the
  // validator would reject must never be emitted, or one deep element would
  // invalidate the whole index.
  const anchorLimit = 220;
  const landmarkTag: Record<string, boolean> = {
    article: true,
    aside: true,
    footer: true,
    h1: true,
    h2: true,
    h3: true,
    h4: true,
    h5: true,
    h6: true,
    header: true,
    main: true,
    nav: true,
    section: true,
  };
  const mediaTag: Record<string, boolean> = {
    canvas: true,
    figure: true,
    img: true,
    picture: true,
    svg: true,
    video: true,
  };
  const controlTag: Record<string, boolean> = {
    a: true,
    button: true,
    input: true,
    select: true,
    textarea: true,
  };
  const controlRole: Record<string, boolean> = { button: true, link: true };
  // Page furniture: never indexed even when a stray attribute would otherwise
  // qualify it. Kept in step with the validator's excluded set.
  const excludedTag: Record<string, boolean> = {
    body: true,
    head: true,
    html: true,
    script: true,
    style: true,
  };

  function collapse(value: string | null): string {
    return (value || "").replace(/\s+/g, " ").trim();
  }

  function clampName(value: string): string {
    return value.slice(0, nameLimit);
  }

  function roleName(tag: string): string {
    if (tag === "header") return "Header";
    if (tag === "nav") return "Navigation";
    if (tag === "main") return "Main";
    if (tag === "aside") return "Aside";
    if (tag === "footer") return "Footer";
    if (tag === "section") return "Section";
    if (tag === "article") return "Article";
    if (tag === "img") return "Image";
    if (tag === "svg") return "Graphic";
    if (tag === "figure") return "Figure";
    if (tag === "picture") return "Picture";
    if (tag === "video") return "Video";
    if (tag === "canvas") return "Canvas";
    return tag;
  }

  function isHeadingTag(tag: string): boolean {
    return (
      tag === "h1" || tag === "h2" || tag === "h3" || tag === "h4" || tag === "h5" || tag === "h6"
    );
  }

  function textInside(element: Element, selector: string): string {
    let found: Element | null = null;
    try {
      found = element.querySelector(selector);
    } catch {
      found = null;
    }
    return found === null ? "" : clampName(collapse(found.textContent));
  }

  function firstHeadingText(element: Element): string {
    return textInside(element, "h1,h2,h3,h4,h5,h6");
  }

  function fileNameFromUrl(value: string | null): string {
    const raw = collapse(value);
    // A data: URL has no file name worth showing, and a very long one is not a
    // name either: both fall through to the id or the role name.
    if (raw === "" || raw.indexOf("data:") === 0 || raw.length > 512) return "";
    const path = raw.split("#")[0].split("?")[0];
    const parts = path.split("/");
    return clampName(parts[parts.length - 1] || "");
  }

  function sourceUrl(element: Element): string {
    const own = element.getAttribute("src");
    if (own !== null && own.trim() !== "") return own;
    let child: Element | null = null;
    try {
      child = element.querySelector("source[src], img[src]");
    } catch {
      child = null;
    }
    return child === null ? "" : child.getAttribute("src") || "";
  }

  function imageAlt(element: Element): string {
    let image: Element | null = element;
    if ((element.tagName || "").toLowerCase() !== "img") {
      try {
        image = element.querySelector("img");
      } catch {
        image = null;
      }
    }
    return image === null ? "" : clampName(collapse(image.getAttribute("alt")));
  }

  /**
   * Media carry no text, so the name is the nearest authored description:
   * aria-label first (ARIA outranks `alt` in the accessible-name computation,
   * and this label is only ever read, never computed), then the element's own
   * naming source — `alt` for an image or a picture, `<title>` for an inline
   * SVG, `<figcaption>` for a figure, the `title` attribute or the file
   * basename for a video. A canvas has no authored text at all and falls
   * through to its id or the spelled role name: its pixels are not a name.
   */
  function mediaText(element: Element, tag: string): string {
    if (tag === "img" || tag === "picture") {
      const alt = imageAlt(element);
      return alt !== "" ? alt : fileNameFromUrl(sourceUrl(element));
    }
    if (tag === "svg") return textInside(element, "title");
    if (tag === "figure") return textInside(element, "figcaption");
    if (tag === "video") {
      const label = clampName(collapse(element.getAttribute("title")));
      return label !== "" ? label : fileNameFromUrl(sourceUrl(element));
    }
    return "";
  }

  /**
   * A landmark's own text is the concatenation of everything inside it (an
   * untitled header reads as the whole page), so it is never the name:
   * landmarks fall back to their first contained heading. Headings, text
   * leaves, and controls keep their own text; media use their naming source.
   * Then aria-label, then the id, then the spelled role name — never empty.
   */
  function nameFor(
    element: Element,
    tag: string,
    kind: string,
    labelled: string,
    id: string,
  ): string {
    if (kind === "landmark") {
      const heading = firstHeadingText(element);
      if (heading !== "") return heading;
    } else if (kind !== "media") {
      const text = clampName(collapse(element.textContent));
      if (text !== "") return text;
    }
    if (labelled !== "") return labelled;
    if (kind === "media") {
      const media = mediaText(element, tag);
      if (media !== "") return media;
    } else if (tag === "input") {
      const value = clampName(collapse(element.getAttribute("value")));
      if (value !== "") return value;
      const placeholder = clampName(collapse(element.getAttribute("placeholder")));
      if (placeholder !== "") return placeholder;
    }
    if (id !== "") return id;
    return roleName(tag);
  }

  function pathFor(element: Element): string {
    const parts: string[] = [];
    let current: Element | null = element;
    let guard = 0;
    // "body[1]" already counts against the budget the validator enforces.
    let length = 6;
    while (current !== null && current !== document.body) {
      if (current === document.documentElement) return "";
      const tag = current.tagName ? current.tagName.toLowerCase() : "";
      if (tag === "") return "";
      let index = 1;
      let sibling = current.previousElementSibling;
      while (sibling !== null) {
        if (sibling.tagName && sibling.tagName.toLowerCase() === tag) index += 1;
        sibling = sibling.previousElementSibling;
      }
      const part = `${tag}[${index}]`;
      length += part.length + 1;
      if (length > anchorLimit) return "";
      parts.push(part);
      current = current.parentElement;
      guard += 1;
      if (guard > 64) return "";
    }
    if (current === null) return "";
    parts.push("body[1]");
    parts.reverse();
    return parts.join("/");
  }

  function depthOf(element: Element): number {
    let depth = 0;
    let current = element.parentElement;
    while (current !== null && current !== document.body && depth < 64) {
      if (current === document.documentElement) return depth;
      depth += 1;
      current = current.parentElement;
    }
    if (current === document.body) depth += 1;
    return depth > 64 ? 64 : depth;
  }

  const sections: ArtifactSection[] = [];
  const seen = new Map<string, boolean>();
  // Element -> index in `sections`, for the nearest-collected-ancestor lookup.
  const indexByElement = new Map<Element, number>();
  const elements = document.querySelectorAll("*");
  for (let listIndex = 0; listIndex < elements.length; listIndex += 1) {
    if (sections.length >= limit) break;
    const element = elements[listIndex] as Element;
    const tag = element.tagName ? element.tagName.toLowerCase() : "";
    if (tag === "" || excludedTag[tag] === true) continue;
    if (document.head !== null && document.head !== undefined && document.head.contains(element)) {
      continue;
    }

    let kind = "";
    if (landmarkTag[tag] === true) kind = isHeadingTag(tag) ? "heading" : "landmark";
    if (kind === "" && mediaTag[tag] === true) kind = "media";
    if (
      kind === "" &&
      (controlTag[tag] === true ||
        element.hasAttribute("tabindex") ||
        controlRole[(element.getAttribute("role") || "").trim().toLowerCase()] === true)
    ) {
      kind = "control";
    }
    if (kind === "" && element.children.length === 0 && collapse(element.textContent) !== "") {
      kind = "text";
    }
    if (kind === "") continue;

    const rect = element.getBoundingClientRect();
    if (
      rect === null ||
      typeof rect.width !== "number" ||
      typeof rect.height !== "number" ||
      !Number.isFinite(rect.left) ||
      !Number.isFinite(rect.top) ||
      !Number.isFinite(rect.width) ||
      !Number.isFinite(rect.height) ||
      rect.width <= 0 ||
      rect.height <= 0
    ) {
      continue;
    }
    let display = "";
    try {
      display = getComputedStyle(element).display;
    } catch {
      display = "";
    }
    if (display === "none") continue;

    const rawId = (element.getAttribute("id") || "").trim();
    const id = rawId.length <= anchorLimit ? rawId : "";
    const anchor = id !== "" ? id : pathFor(element);
    if (anchor === "" || seen.has(anchor)) continue;
    seen.set(anchor, true);

    let parent: number | null = null;
    let ancestor = element.parentElement;
    let ancestorGuard = 0;
    while (ancestor !== null) {
      const collected = indexByElement.get(ancestor);
      if (collected !== undefined) {
        parent = collected;
        break;
      }
      ancestor = ancestor.parentElement;
      ancestorGuard += 1;
      if (ancestorGuard > 128) break;
    }
    indexByElement.set(element, sections.length);

    const labelled = clampName(collapse(element.getAttribute("aria-label")));
    sections.push({
      anchor,
      tag,
      name: nameFor(element, tag, kind, labelled, id),
      depth: depthOf(element),
      parent,
      rect: {
        x: Math.round(rect.left * 100) / 100,
        y: Math.round(rect.top * 100) / 100,
        width: Math.round(rect.width * 100) / 100,
        height: Math.round(rect.height * 100) / 100,
      },
    });
  }
  return sections;
}

/**
 * Generic tag shape accepted from the frame: lowercase, at most 32 characters,
 * no namespaces. The collector emits `element.tagName.toLowerCase()` for every
 * kind, so anything else means a sender that is not the collector and the
 * strict reading of an unknown tag is to reject the list.
 */
const STRUCTURE_TAG_PATTERN = /^[a-z][a-z0-9-]{0,31}$/;
const EXCLUDED_STRUCTURE_TAGS = new Set(["body", "head", "html", "script", "style"]);

function isArtifactStructureTag(tag: string): boolean {
  return STRUCTURE_TAG_PATTERN.test(tag) && !EXCLUDED_STRUCTURE_TAGS.has(tag);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isFiniteCoord(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    value >= -ARTIFACT_STRUCTURE_COORD_BOUND &&
    value <= ARTIFACT_STRUCTURE_COORD_BOUND
  );
}

/**
 * Strict validation for one posted structural entry. Anything outside the
 * bounds rejects the whole list (the caller then treats the structure as
 * absent): the frame is untrusted, and a half-accepted index would mislabel
 * layers and mis-anchor notes. An absent or null `parent` reads as root level;
 * a present one must point at an earlier entry, which is what makes the tree
 * acyclic and the document-order tie-break meaningful.
 */
export function readArtifactStructure(value: unknown): ArtifactSection[] | null {
  if (!Array.isArray(value)) return null;
  if (value.length > MAX_ARTIFACT_STRUCTURE_ENTRIES) return null;
  const sections: ArtifactSection[] = [];
  const seen = new Set<string>();
  for (let index = 0; index < value.length; index += 1) {
    const entryValue = value[index];
    const entry = isRecord(entryValue) ? entryValue : null;
    const rect = entry !== null && isRecord(entry.rect) ? entry.rect : null;
    const parent = entry === null ? undefined : entry.parent;
    const parentValid =
      parent === undefined ||
      parent === null ||
      (typeof parent === "number" && Number.isInteger(parent) && parent >= 0 && parent < index);
    if (
      entry === null ||
      rect === null ||
      !parentValid ||
      typeof entry.anchor !== "string" ||
      entry.anchor.length === 0 ||
      entry.anchor.length > MAX_ARTIFACT_STRUCTURE_ANCHOR_CHARS ||
      typeof entry.tag !== "string" ||
      !isArtifactStructureTag(entry.tag) ||
      typeof entry.name !== "string" ||
      entry.name.length === 0 ||
      entry.name.length > MAX_ARTIFACT_STRUCTURE_NAME_CHARS + 80 ||
      typeof entry.depth !== "number" ||
      !Number.isInteger(entry.depth) ||
      entry.depth < 0 ||
      entry.depth > MAX_ARTIFACT_STRUCTURE_DEPTH ||
      !isFiniteCoord(rect.x) ||
      !isFiniteCoord(rect.y) ||
      typeof rect.width !== "number" ||
      typeof rect.height !== "number" ||
      !Number.isFinite(rect.width) ||
      !Number.isFinite(rect.height) ||
      rect.width < 0 ||
      rect.height < 0 ||
      rect.width > ARTIFACT_STRUCTURE_COORD_BOUND ||
      rect.height > ARTIFACT_STRUCTURE_COORD_BOUND ||
      seen.has(entry.anchor)
    ) {
      return null;
    }
    seen.add(entry.anchor);
    sections.push({
      anchor: entry.anchor,
      tag: entry.tag,
      name: entry.name,
      depth: entry.depth,
      ...(typeof parent === "number" ? { parent } : {}),
      rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
    });
  }
  return sections;
}

/** Prefix every section layer id carries; the anchor follows it verbatim. */
export const SECTION_LAYER_ID_PREFIX = "section:";

/** The id a section layer is built from: the prefix plus the stable anchor. */
export function sectionLayerId(anchor: string): string {
  return `${SECTION_LAYER_ID_PREFIX}${anchor}`;
}

/** The anchor a section layer id was built from, or null for other layers. */
export function sectionAnchorForLayerId(layerId: string): string | null {
  if (!layerId.startsWith(SECTION_LAYER_ID_PREFIX)) return null;
  const anchor = layerId.slice(SECTION_LAYER_ID_PREFIX.length);
  return anchor.length > 0 ? anchor : null;
}

/**
 * Section layers live INSIDE the artifact frame: their canvas transform is the
 * artifact origin plus the measured page rect. The origin must come from the
 * artifact rect (which ignores section layers), never from the sections
 * themselves — that is the cycle break.
 *
 * `parent` (an index in the measured list) is carried through as `parentId`, a
 * layer id, so the panel and the keyboard derive the tree from identities the
 * rest of the code base already handles. The index is only the construction
 * step: the list may be filtered or reordered later, and a stored index would
 * point at the wrong layer the moment it is. A dangling parent index (possible
 * only for a hand-built list, since `readArtifactStructure` rejects one) reads
 * as a root instead of dropping the child.
 */
export function sectionsToLayers(
  sections: readonly ArtifactSection[],
  origin: { x: number; y: number },
): DesignLayer[] {
  const anchorByIndex = sections.map((section) => section.anchor);
  return sections.map((section) => {
    const parentIndex = section.parent;
    const parentAnchor =
      typeof parentIndex === "number" && parentIndex >= 0 && parentIndex < anchorByIndex.length
        ? anchorByIndex[parentIndex]
        : undefined;
    return {
      id: sectionLayerId(section.anchor),
      name: section.name,
      kind: "SECTION" as const,
      transform: {
        x: origin.x + section.rect.x,
        y: origin.y + section.rect.y,
        width: section.rect.width,
        height: section.rect.height,
      },
      section: {
        tag: section.tag,
        anchor: section.anchor,
        ...(parentAnchor === undefined ? {} : { parentId: sectionLayerId(parentAnchor) }),
      },
    };
  });
}

/**
 * Cache of measured indexes, keyed by artifact HTML. Re-measuring on every
 * click is unacceptable (the critic pass costs ~1.5 s), so the surface reads
 * through this cache: the critic feeds it once per new artifact, and panel,
 * canvas, inspector, and notes resolve from it until the artifact changes.
 * Bounded to the most recent artifacts; maps keep insertion order, so the
 * oldest entry is evicted first.
 *
 * What an entry costs, now that MAX_ARTIFACT_STRUCTURE_ENTRIES is 800 rather
 * than the 150 this bound was first chosen against: the key is the whole
 * artifact HTML (up to MAX_ARTIFACT_BYTES, 256 KiB) and the value is the
 * measured index (a list-heavy reference page measures 572 entries at 93 KiB;
 * 800 maximal entries would be 332 KiB). So eight entries retain around 1 MiB
 * on real pages and up to ~4.7 MiB in the worst case either side can produce.
 *
 * Eight stays. The alternative to a cache hit is re-running the critic, which
 * costs ~1.5 s of blocked measurement, and trading a megabyte on a desktop app
 * for that stall is the worse side of the deal. The number to revisit is this
 * one, not the entry cap: raise the cap again and re-read this paragraph.
 */
const MAX_CACHED_ARTIFACT_STRUCTURES = 8;

/** What the critic's single measurement pass reports about one artifact. */
export interface ArtifactStructure {
  readonly sections: readonly ArtifactSection[];
  /**
   * Full page height in CSS px at the canonical page width, from the same
   * pass. Undefined for entries stored before the height was reported: the
   * surface then treats the page as unscrollable.
   */
  readonly contentHeight?: number;
}

const structureCache = new Map<string, ArtifactStructure>();

export function getCachedArtifactStructure(html: string): ArtifactStructure | undefined {
  return structureCache.get(html);
}

export function getCachedArtifactSections(html: string): readonly ArtifactSection[] | undefined {
  return structureCache.get(html)?.sections;
}

export function setCachedArtifactStructure(html: string, structure: ArtifactStructure): void {
  if (structureCache.has(html)) structureCache.delete(html);
  structureCache.set(html, structure);
  while (structureCache.size > MAX_CACHED_ARTIFACT_STRUCTURES) {
    const oldest = structureCache.keys().next();
    if (oldest.done) break;
    structureCache.delete(oldest.value);
  }
}

export function setCachedArtifactSections(
  html: string,
  sections: readonly ArtifactSection[],
  contentHeight?: number,
): void {
  setCachedArtifactStructure(html, {
    sections,
    ...(contentHeight === undefined ? {} : { contentHeight }),
  });
}

export function clearCachedArtifactSections(): void {
  structureCache.clear();
}
