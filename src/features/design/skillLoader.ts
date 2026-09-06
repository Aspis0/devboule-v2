// ── Design-skill loader ───────────────────────────────────────────────
//
// A skill section is a markdown file with YAML-like front-matter:
//
//   ---
//   slug: typography
//   title: Typography
//   requires: [color]
//   ---
//
// Three fields only: slug (identifier, matches filename), title (display),
// requires (other slugs this section assumes, optional = none).
//
// Two entry points over the same machinery:
//   buildSkillBlock  — tolerant runtime, skips failures, always returns a block
//   validateSections — strict first-party check, fails on any problem
//
// Characters are a proxy for tokens, deliberately conservative.  No tokeniser
// is added for this — ~4 chars/token is a safe estimate for English prose.

// ── Types ──────────────────────────────────────────────────────────────

export interface SkillSection {
  slug: string;
  title: string;
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

export type SectionError = { kind: "section_too_large"; slug: string; length: number };

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

/** Character ceiling for the composed doctrine block (~1000 tokens). */
export const DOCTRINE_CEILING_CHARS = 4000;

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
  let requires: string[] | null = null;
  const seen = new Set<string>();

  for (const line of frontLines) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#")) continue;

    const colonIndex = trimmed.indexOf(":");
    if (colonIndex === -1) {
      return {
        ok: false,
        error: { kind: "malformed_front_matter", detail: `Unparseable line: ${trimmed}` },
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
    } else if (key === "requires") {
      requires = parseRequiresValue(value);
    }
    // Unknown keys are silently ignored (forward compatibility).
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
    section: { slug, title, requires: requires ?? [], body },
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
 *   3. Ties break alphabetically by slug.
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
  const nodes = [...closure.entries()].map(([slug, info]) => ({
    slug,
    section: bySlug.get(slug)!,
    depth: info.depth,
    explicit: info.explicit,
  }));

  nodes.sort((a, b) => {
    if (a.explicit !== b.explicit) return a.explicit ? -1 : 1;
    if (a.depth !== b.depth) return a.depth - b.depth;
    return a.slug.localeCompare(b.slug);
  });

  return { ok: true, order: nodes.map((n) => n.section) };
}

// ── Composition ────────────────────────────────────────────────────────

const SECTION_SEPARATOR = "\n\n---\n\n";

/**
 * Compose an ordered list of sections into one block under a character
 * ceiling. Sections that do not fit are dropped from the end and listed
 * in `dropped`.
 *
 * A single section that alone exceeds the ceiling is still included —
 * the ceiling is a proxy, not a hard limit, and a coherent section is
 * more valuable than an empty block.  The `dropped` list names only
 * sections that were explicitly cut because there was no room after
 * including earlier ones.
 */
export function composeSections(
  sections: readonly SkillSection[],
  ceiling = DOCTRINE_CEILING_CHARS,
): ComposedBlock {
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

  const text = parts.join(SECTION_SEPARATOR);
  return { text, dropped, totalChars: text.length, ceiling };
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
 * slugs, unknown `requires`, cycles, and sections whose body alone
 * exceeds the doctrine ceiling.
 * Returns every problem found rather than stopping at the first.
 */
export function validateSections(sources: readonly SkillSource[]): readonly ValidationError[] {
  const errors: ValidationError[] = [];
  const { sections, errors: parseErrors } = parseSources(sources);

  for (const pe of parseErrors) {
    errors.push({ path: pe.path, error: pe.error });
  }

  if (sections.length === 0) return errors;

  // Check every section body fits within the ceiling.
  for (const section of sections) {
    if (section.body.length > DOCTRINE_CEILING_CHARS) {
      errors.push({
        path: section.slug,
        error: {
          kind: "section_too_large",
          slug: section.slug,
          length: section.body.length,
        },
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
