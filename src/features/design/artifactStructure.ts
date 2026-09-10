import type { DesignLayer } from "./designHost";

/**
 * Structural index of a generated page: landmarks and headings measured in the
 * render critic's off-screen frame, in the same single pass as the render
 * checks. The display iframe is unreadable from the parent (`sandbox=""`,
 * `script-src 'none'`, `inert`), so this index is the only thing that feeds
 * the Layers panel for a generated artifact.
 *
 * Only landmarks (`header`, `nav`, `main`, `aside`, `footer`, `section`,
 * `article`) and headings (`h1`-`h6`) are collected: the panel must stay
 * readable, not become a dump of the DOM.
 */

/** Tags collected into the structural index, in selector order. */
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

export type ArtifactStructureTag = (typeof ARTIFACT_STRUCTURE_TAGS)[number];

/** Selector the frame-side collector walks. Keep in sync with the tags above. */
export const ARTIFACT_STRUCTURE_SELECTOR =
  "header,nav,main,aside,footer,section,article,h1,h2,h3,h4,h5,h6";

/** Hard cap on collected elements per measurement; the panel is a summary. */
export const MAX_ARTIFACT_STRUCTURE_ENTRIES = 150;

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
   * silently dropped one.
   */
  anchor: string;
  tag: ArtifactStructureTag;
  /** Readable name: headings keep their text; landmarks use their first
   * contained heading, then aria-label, then id, then the role name. */
  name: string;
  /** Nesting depth below <body>: a direct child of <body> is 1. */
  depth: number;
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
  const selector = "header,nav,main,aside,footer,section,article,h1,h2,h3,h4,h5,h6";
  const limit = 150;
  const nameLimit = 80;

  function collapse(value: string | null): string {
    return (value || "").replace(/\s+/g, " ").trim();
  }

  function landmarkRoleName(tag: string): string {
    if (tag === "header") return "Header";
    if (tag === "nav") return "Navigation";
    if (tag === "main") return "Main";
    if (tag === "aside") return "Aside";
    if (tag === "footer") return "Footer";
    if (tag === "section") return "Section";
    if (tag === "article") return "Article";
    return tag;
  }

  function firstHeadingText(element: Element): string {
    let heading: Element | null = null;
    try {
      heading = element.querySelector("h1,h2,h3,h4,h5,h6");
    } catch {
      heading = null;
    }
    if (heading === null) return "";
    return collapse(heading.textContent).slice(0, nameLimit);
  }

  function isHeadingTag(tag: string): boolean {
    return (
      tag === "h1" ||
      tag === "h2" ||
      tag === "h3" ||
      tag === "h4" ||
      tag === "h5" ||
      tag === "h6"
    );
  }
  function pathFor(element: Element): string {
    const parts: string[] = [];
    let current: Element | null = element;
    let guard = 0;
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
      parts.push(`${tag}[${index}]`);
      current = current.parentElement;
      guard += 1;
      if (guard > 64 || parts.length > 32) return "";
    }
    if (current === null) return "";
    parts.push("body[1]");
    parts.reverse();
    return parts.join("/");
  }

  function pathDepth(element: Element): number {
    let depth = 0;
    let current = element.parentElement;
    let guard = 0;
    while (current !== null && current !== document.body) {
      if (current === document.documentElement) return depth;
      depth += 1;
      current = current.parentElement;
      guard += 1;
      if (guard > 64) return depth;
    }
    return current === null ? depth : depth + 1;
  }

  const sections: ArtifactSection[] = [];
  const seen: Record<string, true> = {};
  const elements = document.querySelectorAll(selector);
  for (let listIndex = 0; listIndex < elements.length; listIndex += 1) {
    if (sections.length >= limit) break;
    const element = elements[listIndex] as Element;
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
    const tag = element.tagName.toLowerCase();
    const id = (element.getAttribute("id") || "").trim();
    const anchor = id !== "" ? id : pathFor(element);
    if (anchor === "" || seen[anchor] === true) continue;
    seen[anchor] = true;
    const text = collapse(element.textContent).slice(0, nameLimit);
    const labelled = collapse(element.getAttribute("aria-label")).slice(0, nameLimit);
    // A landmark's own text is the concatenation of everything inside it
    // (an untitled header reads as the whole page), so it is never the name:
    // landmarks fall back to their first contained heading, then aria-label,
    // then id, then the role name. Headings keep their own text.
    let name: string;
    if (isHeadingTag(tag)) {
      name = text !== "" ? text : labelled !== "" ? labelled : id !== "" ? id : tag;
    } else {
      const headingText = firstHeadingText(element);
      const role = landmarkRoleName(tag);
      name =
        headingText !== ""
          ? headingText
          : labelled !== ""
            ? labelled
            : id !== ""
              ? id
              : role;
    }
    // Depth below <body>: pathFor counts body[1] as the root, so parts minus body.
    const depth = anchor === id ? pathDepth(element) : anchor.split("/").length - 1;
    sections.push({
      anchor,
      tag: tag as ArtifactSection["tag"],
      name,
      depth,
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

const STRUCTURE_TAG_SET = new Set<string>(ARTIFACT_STRUCTURE_TAGS);

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
 * layers and mis-anchor notes.
 */
export function readArtifactStructure(value: unknown): ArtifactSection[] | null {
  if (!Array.isArray(value)) return null;
  if (value.length > MAX_ARTIFACT_STRUCTURE_ENTRIES) return null;
  const sections: ArtifactSection[] = [];
  const seen = new Set<string>();
  for (const entryValue of value) {
    const entry = isRecord(entryValue) ? entryValue : null;
    const rect = entry !== null && isRecord(entry.rect) ? entry.rect : null;
    if (
      entry === null ||
      rect === null ||
      typeof entry.anchor !== "string" ||
      entry.anchor.length === 0 ||
      entry.anchor.length > MAX_ARTIFACT_STRUCTURE_ANCHOR_CHARS ||
      typeof entry.tag !== "string" ||
      !STRUCTURE_TAG_SET.has(entry.tag) ||
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
      tag: entry.tag as ArtifactSection["tag"],
      name: entry.name,
      depth: entry.depth,
      rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
    });
  }
  return sections;
}

/** Prefix every section layer id carries; the anchor follows it verbatim. */
export const SECTION_LAYER_ID_PREFIX = "section:";

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
 */
export function sectionsToLayers(
  sections: readonly ArtifactSection[],
  origin: { x: number; y: number },
): DesignLayer[] {
  return sections.map((section) => ({
    id: `${SECTION_LAYER_ID_PREFIX}${section.anchor}`,
    name: section.name,
    kind: "SECTION" as const,
    transform: {
      x: origin.x + section.rect.x,
      y: origin.y + section.rect.y,
      width: section.rect.width,
      height: section.rect.height,
    },
    section: { tag: section.tag, anchor: section.anchor },
  }));
}

/**
 * Cache of measured indexes, keyed by artifact HTML. Re-measuring on every
 * click is unacceptable (the critic pass costs ~1.5 s), so the surface reads
 * through this cache: the critic feeds it once per new artifact, and panel,
 * canvas, inspector, and notes resolve from it until the artifact changes.
 * Bounded to the most recent artifacts; maps keep insertion order, so the
 * oldest entry is evicted first.
 */
const MAX_CACHED_ARTIFACT_STRUCTURES = 8;

const structureCache = new Map<string, readonly ArtifactSection[]>();

export function getCachedArtifactSections(html: string): readonly ArtifactSection[] | undefined {
  return structureCache.get(html);
}

export function setCachedArtifactSections(
  html: string,
  sections: readonly ArtifactSection[],
): void {
  if (structureCache.has(html)) structureCache.delete(html);
  structureCache.set(html, sections);
  while (structureCache.size > MAX_CACHED_ARTIFACT_STRUCTURES) {
    const oldest = structureCache.keys().next();
    if (oldest.done) break;
    structureCache.delete(oldest.value);
  }
}

export function clearCachedArtifactSections(): void {
  structureCache.clear();
}
