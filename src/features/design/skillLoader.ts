// ── Design-skill loader ───────────────────────────────────────────────
//
// A skill section is a markdown file with YAML-like front-matter:
//
//   ---
//   slug: typography
//   title: Typography
//   description: Rules for making type feel deliberate.
//   requires: [color]
//   ---
//
// Four fields only: slug (identifier, matches filename), title (display),
// description (short relevance guidance), and requires (other slugs this
// section assumes, optional = none).
//
// Two entry points over the same machinery:
//   buildSkillBlock  — tolerant runtime, skips failures, always returns a block
//   validateSections — strict first-party check, fails on any problem
//
// Characters are a proxy for tokens, deliberately conservative.  No tokeniser
// is added for this — ~4 chars/token is a safe estimate for English prose. The
// 12000-character composed ceiling is about 3000 tokens; the 2500-character section
// ceiling keeps one section from monopolising the block, and is binding rather than
// generous, so it forces condensation instead of permitting it.
//
// 12000 is borrowed, not derived. It is the only published hard character cap found in a
// comparable product: Windsurf limits a workspace rule file to 12000 characters and a
// global rule file to 6000. The rest publish lines — Cursor under 500, GitHub Copilot
// roughly 20-50 for custom instructions and about 1000 for a review file, Claude Code
// under 200 per CLAUDE.md with the accompanying claim that longer files reduce adherence.
// Nobody publishes a measured optimum, and this is not one either.
//
// The corpus is deliberately larger than the ceiling. nexu-io/open-design ships about
// 112000 characters of the same kind of doctrine with no budget and no truncation at all,
// because each skill declares the sections it needs and only those are injected. Selection
// is what makes a large library affordable; the ceiling only decides what happens when
// selection is refused.
//
// Instruction count is what the density literature measures, and it is deliberately not
// the unit here. Those instructions are atomic keyword inclusions while these are
// interdependent principles, so the two do not share a scale; and counting directives in
// prose needs a heuristic that misfires in both directions. Characters are exact, and they
// are what the block costs on every request.

// ── Types ──────────────────────────────────────────────────────────────

export interface SkillSection {
  slug: string;
  title: string;
  description: string;
  unknownKeys: readonly string[];
  requires: readonly string[];
  body: string;
}

export interface SkillSource {
  path: string;
  text: string;
}

export type ParseError =
  | { kind: "malformed_front_matter"; detail: string }
  | { kind: "invalid_slug"; slug: string }
  | { kind: "slug_filename_mismatch"; slug: string; filename: string }
  | { kind: "duplicate_key"; key: string };

export type ParseResult = { ok: true; section: SkillSection } | { ok: false; error: ParseError };

export type ResolveError =
  | { kind: "cycle"; path: readonly string[] }
  | { kind: "unknown_slug"; slug: string };

export type ResolveResult =
  | { ok: true; order: readonly SkillSection[] }
  | { ok: false; error: ResolveError };

export type SectionError =
  | { kind: "section_too_large"; slug: string; length: number }
  | { kind: "section_empty"; slug: string }
  | { kind: "description_too_long"; slug: string; length: number }
  | { kind: "unknown_front_matter_key"; slug: string; key: string };

export interface ComposedBlock {
  text: string;
  dropped: readonly string[];
  totalChars: number;
  ceiling: number;
}

export interface ValidationError {
  path: string;
  error: ParseError | ResolveError | SectionError;
}

// ── Constants ──────────────────────────────────────────────────────────

/** Character ceiling for the composed doctrine block (~3000 tokens). See the header note. */
export const DOCTRINE_CEILING_CHARS = 12000;

/** Strict per-section ceiling: a section outgrowing it is two sections, not a bigger number. */
export const DOCTRINE_SECTION_CEILING_CHARS = 2500;

/** Strict character ceiling for relevance metadata; real descriptions are about 270 chars. */
export const DOCTRINE_DESCRIPTION_CEILING_CHARS = 300;

const SLUG_PATTERN = /^[a-z][a-z0-9-]*$/;

// ── Front-matter parsing ───────────────────────────────────────────────

function parseSlugValue(raw: string): string | null {
  const trimmed = raw.trim();
  return SLUG_PATTERN.test(trimmed) ? trimmed : null;
}

function parseRequiresValue(raw: string): string[] {
  const trimmed = raw.trim();
  if (trimmed === "" || trimmed === "[]") return [];
  const inner = trimmed.replace(/^\[|\]$/g, "").trim();
  if (inner === "") return [];
  return inner
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/**
 * Parse one skill file's text into a `SkillSection` or a typed failure.
 *
 * Distinct failures (all distinguishable):
 *   malformed_front_matter — missing/broken --- fences or unparseable keys
 *   invalid_slug           — slug fails ^[a-z][a-z0-9-]*$
 *   slug_filename_mismatch — slug does not equal the stem of `path`
 *   duplicate_key          — the same key appears twice in one file
 */
export function parseSkillFile(path: string, text: string): ParseResult {
  const lines = text.split("\n");
  if (lines.length < 2 || lines[0]!.trim() !== "---") {
    return {
      ok: false,
      error: { kind: "malformed_front_matter", detail: "Missing opening ---" },
    };
  }

  let endIndex = -1;
  for (let i = 1; i < lines.length; i++) {
    if (lines[i]!.trim() === "---") {
      endIndex = i;
      break;
    }
  }
  if (endIndex === -1) {
    return {
      ok: false,
      error: { kind: "malformed_front_matter", detail: "Missing closing ---" },
    };
  }

  const frontLines = lines.slice(1, endIndex);
  const body = lines
    .slice(endIndex + 1)
    .join("\n")
    .trim();

  let slug: string | null = null;
  let title: string | null = null;
  let description: string | null = null;
  const unknownKeys: string[] = [];
  let requires: string[] | null = null;
  const seen = new Set<string>();

  for (const line of frontLines) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#")) continue;

    const colonIndex = trimmed.indexOf(":");
    if (colonIndex === -1) {
      return {
        ok: false,
        error: {
          kind: "malformed_front_matter",
          // Every front-matter line must carry its own key, so a value that
          // wraps onto a second line lands here.  Say so: the ceiling allows a
          // 300-character description, which invites exactly that wrap.
          detail: `Unparseable line, a front-matter value must stay on one line: ${trimmed}`,
        },
      };
    }

    const key = trimmed.slice(0, colonIndex).trim();
    const value = trimmed.slice(colonIndex + 1).trim();

    if (seen.has(key)) {
      return { ok: false, error: { kind: "duplicate_key", key } };
    }
    seen.add(key);

    if (key === "slug") {
      const parsed = parseSlugValue(value);
      if (parsed === null) {
        return { ok: false, error: { kind: "invalid_slug", slug: value } };
      }
      slug = parsed;
    } else if (key === "title") {
      title = value;
    } else if (key === "description") {
      description = value;
    } else if (key === "requires") {
      requires = parseRequiresValue(value);
    } else {
      // Retain unknown keys deliberately: the tolerant runtime ignores them,
      // while validateSections consumes them for strict first-party checks.
      unknownKeys.push(key);
    }
  }

  if (slug === null) {
    return {
      ok: false,
      error: { kind: "malformed_front_matter", detail: "Missing slug field" },
    };
  }
  if (title === null) {
    return {
      ok: false,
      error: { kind: "malformed_front_matter", detail: "Missing title field" },
    };
  }
  if (description === null) {
    return {
      ok: false,
      error: { kind: "malformed_front_matter", detail: "Missing description field" },
    };
  }

  // Check slug matches the filename stem.
  const filename = path.split("/").pop()?.split(".")[0];
  if (filename !== undefined && slug !== filename) {
    return {
      ok: false,
      error: { kind: "slug_filename_mismatch", slug, filename },
    };
  }

  return {
    ok: true,
    section: { slug, title, description, unknownKeys, requires: requires ?? [], body },
  };
}

// ── Source loading ─────────────────────────────────────────────────────

function parseSources(sources: readonly SkillSource[]): {
  sections: SkillSection[];
  errors: Array<{ path: string; error: ParseError }>;
} {
  const sections: SkillSection[] = [];
  const errors: Array<{ path: string; error: ParseError }> = [];
  const seen = new Map<string, string>(); // slug → first path

  for (const source of sources) {
    const result = parseSkillFile(source.path, source.text);
    if (!result.ok) {
      errors.push({ path: source.path, error: result.error });
      continue;
    }

    const existing = seen.get(result.section.slug);
    if (existing !== undefined) {
      errors.push({
        path: source.path,
        error: { kind: "duplicate_key", key: result.section.slug },
      });
      continue;
    }
    seen.set(result.section.slug, source.path);
    sections.push(result.section);
  }

  return { sections, errors };
}

// ── Dependency resolution ──────────────────────────────────────────────

class CycleError extends Error {
  path: string[];

  constructor(path: string[]) {
    super(`Cycle detected: ${path.join(" → ")}`);
    this.path = path;
  }
}

/**
 * Resolve a set of requested slugs into an ordered list, closing over
 * `requires` dependencies.
 *
 * Ordering (derived, no priority field):
 *   1. Explicitly requested slugs outrank pulled-in dependencies.
 *   2. Within a tier, shallower dependency depth first.
 *   3. Ties break by the order the slugs were requested. Dependencies that
 *      were not requested use alphabetical order as their deterministic fallback.
 *
 * Unknown `requires` slugs are tolerated (skipped). Cycles are detected
 * and returned as a `cycle` error with the path found.
 */
export function resolveSections(
  requested: readonly string[],
  sections: readonly SkillSection[],
): ResolveResult {
  const bySlug = new Map<string, SkillSection>();
  for (const s of sections) bySlug.set(s.slug, s);

  // Every requested slug must exist.
  for (const slug of requested) {
    if (!bySlug.has(slug)) {
      return { ok: false, error: { kind: "unknown_slug", slug } };
    }
  }

  // DFS closure with cycle detection.
  const visited = new Set<string>();
  const stack: string[] = [];
  const closure = new Map<string, { depth: number; explicit: boolean }>();

  const visit = (slug: string, depth: number, explicit: boolean): void => {
    if (stack.includes(slug)) {
      const cyclePath = [...stack.slice(stack.indexOf(slug)), slug];
      throw new CycleError(cyclePath);
    }
    if (visited.has(slug)) {
      const existing = closure.get(slug);
      if (existing !== undefined && depth < existing.depth) {
        closure.set(slug, {
          depth,
          explicit: explicit || existing.explicit,
        });
      }
      return;
    }

    visited.add(slug);
    stack.push(slug);

    const section = bySlug.get(slug);
    if (section === undefined) {
      // Unknown slug in requires — tolerated.
      stack.pop();
      return;
    }

    const existing = closure.get(slug);
    if (
      existing === undefined ||
      depth < existing.depth ||
      (depth === existing.depth && explicit && !existing.explicit)
    ) {
      closure.set(slug, {
        depth,
        explicit: explicit || (existing?.explicit ?? false),
      });
    }

    for (const dep of section.requires) {
      if (bySlug.has(dep)) visit(dep, depth + 1, false);
    }

    stack.pop();
  };

  try {
    for (const slug of requested) visit(slug, 0, true);
  } catch (e) {
    if (e instanceof CycleError) {
      return { ok: false, error: { kind: "cycle", path: e.path } };
    }
    throw e;
  }

  // Build and sort.
  const requestedOrder = new Map<string, number>();
  requested.forEach((slug, index) => {
    if (!requestedOrder.has(slug)) requestedOrder.set(slug, index);
  });
  const nodes = [...closure.entries()].map(([slug, info]) => ({
    slug,
    section: bySlug.get(slug)!,
    depth: info.depth,
    explicit: info.explicit,
  }));

  nodes.sort((a, b) => {
    if (a.explicit !== b.explicit) return a.explicit ? -1 : 1;
    if (a.depth !== b.depth) return a.depth - b.depth;
    const aRequested = requestedOrder.get(a.slug);
    const bRequested = requestedOrder.get(b.slug);
    if (aRequested !== undefined && bRequested !== undefined) return aRequested - bRequested;
    return a.slug.localeCompare(b.slug);
  });

  return { ok: true, order: nodes.map((n) => n.section) };
}

// ── Composition ────────────────────────────────────────────────────────

export const SECTION_SEPARATOR = "\n\n---\n\n";

export const TRUNCATION_NOTICE =
  "Some doctrine sections were omitted to fit the budget; this block is not the complete doctrine.";

function fitSections(
  sections: readonly SkillSection[],
  ceiling: number,
): { parts: string[]; dropped: string[] } {
  const dropped: string[] = [];
  const parts: string[] = [];

  for (const section of sections) {
    const block = `## ${section.title}\n\n${section.body}`;
    const candidate =
      parts.length === 0 ? block : `${parts.join(SECTION_SEPARATOR)}${SECTION_SEPARATOR}${block}`;

    if (candidate.length > ceiling) {
      if (parts.length === 0) {
        // First (only) section alone exceeds the ceiling — include it anyway.
        parts.push(block);
      } else {
        dropped.push(section.slug);
      }
    } else {
      parts.push(block);
    }
  }

  return { parts, dropped };
}

/**
 * Compose an ordered list of sections into one block under a character
 * ceiling. Sections that do not fit are dropped from the end and listed
 * in `dropped`. If anything is dropped, the returned block includes a notice
 * that the doctrine is incomplete.
 *
 * A single section that alone exceeds the ceiling is still included —
 * this is the one case where the returned text can exceed `ceiling`. The
 * ceiling is a proxy, not a hard limit, and a coherent section is more
 * valuable than an empty block. The `dropped` list names only sections that
 * were explicitly cut because there was no room after including earlier ones.
 */
export function composeSections(
  sections: readonly SkillSection[],
  ceiling = DOCTRINE_CEILING_CHARS,
): ComposedBlock {
  const firstFit = fitSections(sections, ceiling);
  if (firstFit.dropped.length === 0) {
    const text = firstFit.parts.join(SECTION_SEPARATOR);
    return { text, dropped: [], totalChars: text.length, ceiling };
  }

  const reservedCeiling = ceiling - (SECTION_SEPARATOR.length + TRUNCATION_NOTICE.length);
  const finalFit = fitSections(sections, reservedCeiling);
  const text = [...finalFit.parts, TRUNCATION_NOTICE].join(SECTION_SEPARATOR);
  return { text, dropped: finalFit.dropped, totalChars: text.length, ceiling };
}

// ── Tolerant runtime ───────────────────────────────────────────────────

/**
 * Build a composed doctrine block from sources.
 *
 * Tolerant: malformed files, unknown `requires` slugs and cycles are
 * silently handled — the call always gets a usable block.  An empty
 * `sources` or empty `requested` produces an empty block (empty string,
 * no dropped sections).
 *
 * When a cycle is detected, the sections involved in the cycle are
 * excluded and the remaining non-cyclic sections are still composed.
 * The retry loop is bounded by the number of sections: each pass removes
 * at least one section (the cycle path), so it cannot spin.
 */
export function buildSkillBlock(
  sources: readonly SkillSource[],
  requested: readonly string[],
): ComposedBlock {
  if (requested.length === 0) {
    return { text: "", dropped: [], totalChars: 0, ceiling: DOCTRINE_CEILING_CHARS };
  }

  const { sections } = parseSources(sources);
  if (sections.length === 0) {
    return { text: "", dropped: [], totalChars: 0, ceiling: DOCTRINE_CEILING_CHARS };
  }

  const EMPTY: ComposedBlock = {
    text: "",
    dropped: [],
    totalChars: 0,
    ceiling: DOCTRINE_CEILING_CHARS,
  };

  let currentSections = sections;
  let currentRequested = [...requested];

  // Bound: each pass removes at least one section, so at most sections.length
  // iterations.  A pass that removes nothing means no progress is possible.
  for (let attempt = 0; attempt < sections.length; attempt++) {
    const result = resolveSections(currentRequested, currentSections);
    if (result.ok) return composeSections(result.order);

    if (result.error.kind !== "cycle") {
      // Unknown slug — unrecoverable.
      return EMPTY;
    }

    const exclude = new Set(result.error.path);
    const nextSections = currentSections.filter((s) => !exclude.has(s.slug));
    const nextRequested = currentRequested.filter((s) => !exclude.has(s));

    if (nextSections.length === currentSections.length) {
      // No progress — a cycle whose path is not in sections cannot be broken.
      return EMPTY;
    }
    if (nextRequested.length === 0) {
      return EMPTY;
    }

    currentSections = nextSections;
    currentRequested = nextRequested;
  }

  return EMPTY;
}

// ── Strict first-party check ───────────────────────────────────────────

/**
 * Validate all sources as first-party content.
 *
 * Fails on: malformed front-matter, slug/filename mismatch, duplicate
 * slugs, unknown `requires`, cycles, and sections whose body alone exceeds
 * the section ceiling.
 * Returns every problem found rather than stopping at the first.
 */
export function validateSections(sources: readonly SkillSource[]): readonly ValidationError[] {
  const errors: ValidationError[] = [];
  const { sections, errors: parseErrors } = parseSources(sources);

  for (const pe of parseErrors) {
    errors.push({ path: pe.path, error: pe.error });
  }

  if (sections.length === 0) return errors;

  // Check every section body fits within the per-section ceiling.
  for (const section of sections) {
    if (section.body.length === 0) {
      errors.push({
        path: section.slug,
        error: { kind: "section_empty", slug: section.slug },
      });
    } else if (section.body.length > DOCTRINE_SECTION_CEILING_CHARS) {
      errors.push({
        path: section.slug,
        error: {
          kind: "section_too_large",
          slug: section.slug,
          length: section.body.length,
        },
      });
    }
    if (section.description.length > DOCTRINE_DESCRIPTION_CEILING_CHARS) {
      errors.push({
        path: section.slug,
        error: {
          kind: "description_too_long",
          slug: section.slug,
          length: section.description.length,
        },
      });
    }
    for (const key of section.unknownKeys) {
      errors.push({
        path: section.slug,
        error: { kind: "unknown_front_matter_key", slug: section.slug, key },
      });
    }
  }

  // Check every requires slug exists.
  const bySlug = new Map<string, SkillSection>();
  for (const s of sections) bySlug.set(s.slug, s);

  for (const section of sections) {
    for (const dep of section.requires) {
      if (!bySlug.has(dep)) {
        errors.push({
          path: `(requires of ${section.slug})`,
          error: { kind: "unknown_slug" as const, slug: dep },
        });
      }
    }
  }

  // Check for cycles.
  const requested = sections.map((s) => s.slug);
  const resolveResult = resolveSections(requested, sections);
  if (!resolveResult.ok) {
    errors.push({
      path: "(dependency graph)",
      error: resolveResult.error,
    });
  }

  return errors;
}
