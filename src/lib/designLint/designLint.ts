// Design lint for Devboule TSX and CSS sources.
//
// Pure functions over source strings: no DOM, no React, no filesystem.
// Every rule takes text and returns findings, so each one is trivially
// unit-testable. The idea catalogue (which patterns are worth checking)
// is adapted from OpenDesign's Apache-2.0 `lint-artifact.ts`
// (apps/daemon/src/lint-artifact.ts) and craft/anti-ai-slop.md; the
// rules themselves are written for TSX/CSS source, not ported. Where a
// pattern list is taken near-verbatim from upstream it is marked below.
//
// Tolerances by design: the linter does not parse JSX or CSS fully.
// It is greppy on purpose — cheap, deterministic, and it may miss
// exotic nesting. It must not, however, invent findings on ordinary
// code; the negative tests in designLint.test.ts pin that.

export interface DesignLintFinding {
  rule: string; // kebab-case id
  severity: "error" | "warning";
  line: number; // 1-indexed
  column?: number; // 1-indexed, when known
  message: string; // what is wrong and what to do instead
}

/** Normalized (lowercase, whitespace-stripped) colour value -> token name. */
export type DesignTokenMap = Record<string, string>;

/** Colour values that read as "unedited LLM accent". Upstream: craft/anti-ai-slop.md
 * cardinal sin #1, kept verbatim (also AI_DEFAULT_INDIGO in lint-artifact.ts). */
const AI_DEFAULT_INDIGO = [
  "#6366f1",
  "#4f46e5",
  "#4338ca",
  "#3730a3",
  "#8b5cf6",
  "#7c3aed",
  "#a855f7",
];
const AI_INDIGO = new Set(AI_DEFAULT_INDIGO.map(normalizeValue));

// Stop colours for the "trust gradient" families (craft/anti-ai-slop.md
// cardinal sin #2). Hex ramps adapted from OpenDesign lint-artifact.ts.
const GRADIENT_FAMILIES: Record<string, string[]> = {
  purple: ["#a855f7", "#9333ea", "#7c3aed", "#6d28d9", "#581c87", "#8b5cf6", "#a78bfa"],
  indigo: ["#6366f1", "#4f46e5", "#4338ca", "#3730a3", "#312e81", "#818cf8", "#a5b4fc"],
  blue: ["#3b82f6", "#2563eb", "#1d4ed8", "#1e40af", "#1e3a8a", "#60a5fa", "#93c5fd"],
  cyan: ["#06b6d4", "#0891b2", "#0e7490", "#155e75", "#164e63", "#22d3ee", "#67e8f9"],
  pink: ["#ec4899", "#db2777", "#be185d", "#9d174d", "#831843", "#f472b6", "#f9a8d4"],
};
const GRADIENT_KEYWORDS: Record<string, string> = {
  purple: "purple",
  violet: "purple",
  indigo: "indigo",
  blue: "blue",
  cyan: "cyan",
  pink: "pink",
  fuchsia: "pink",
  magenta: "pink",
};
// The documented two-stop families: purple→blue, blue→cyan, indigo→pink.
const FORBIDDEN_GRADIENT_PAIRS: Array<[string, string]> = [
  ["purple", "blue"],
  ["indigo", "blue"],
  ["indigo", "pink"],
  ["blue", "cyan"],
];

// Filler copy phrases (anti-slop cardinal sin #7; list adapted from
// upstream FILLER_PATTERNS in lint-artifact.ts).
const FILLER_PATTERNS = [
  /\blorem\s+ipsum\b/i,
  /\bdolor\s+sit\s+amet\b/i,
  /\bplaceholder\s+text\b/i,
  /\bsample\s+content\b/i,
  /\bfeature\s+(?:one|two|three|1|2|3)\b/i,
];

// Invented marketing numbers (anti-slop cardinal sin #6; shapes adapted
// from upstream INVENTED_METRIC_PATTERNS, extended to the ASCII "10x" form).
const METRIC_PATTERNS = [
  /\b\d+(?:\.\d+)?\s*[×x]\s+(?:faster|better|easier|stronger|smoother)\b/i,
  /\b\d+(?:\.\d+)?\s*[×x]\s+more\s+(?:productive|efficient|effective)\b/i,
  /\b\d{2,3}(?:\.\d+)?%\s+uptime\b/i,
  /\bzero[- ]downtime\b/i,
];

const EMOJI_RE = /\p{Extended_Pictographic}/gu;
// Typographic symbols that are real UI, not emoji decoration.
const EMOJI_ALLOWLIST = new Set(["✓", "✔", "✗", "✘", "×", "⌘"]);

const ICON_TAGS = new Set(["h1", "h2", "h3", "h4", "h5", "h6", "button", "li"]);
const VOID_TAGS = new Set([
  "area",
  "base",
  "br",
  "col",
  "embed",
  "hr",
  "img",
  "input",
  "link",
  "meta",
  "param",
  "source",
  "track",
  "wbr",
]);

const HEX_RE = /#[0-9a-f]{3,8}\b/gi;
// A `#` literal is only a colour when it sits where a CSS value sits.
// `#412` in "Open #412 on GitHub" is a PR number, not a colour.
function looksLikeColourValue(source: string, end: number): boolean {
  const next = source[end];
  if (next === undefined) return true;
  if (";,)]}\"'".includes(next)) return true;
  if (/\s/.test(next)) {
    const after = source.slice(end).match(/^\s+(.)/);
    // Whitespace then anything that is not a letter keeps value position
    // (`#fff\n}`, `#fff !important`); whitespace then a letter reads as
    // prose (`#412 on GitHub`).
    return after !== null && !/[a-z]/i.test(after[1] ?? "");
  }
  return false;
}
const FUNC_COLOUR_RE = /\b(?:rgba?|hsla?)\(\s*\d/g;
const GRADIENT_RE = /\blinear-gradient\s*\(([^()]*)\)/gi;
const CSS_COMMENT_RE = /\/\*[\s\S]*?\*\//g;
const TOKEN_DECL_RE = /(--[\w-]+)\s*:\s*([^;]+);/g;

function normalizeValue(value: string): string {
  return value.toLowerCase().replace(/\s+/g, "");
}

function isGlobalCss(filename: string): boolean {
  return /(^|[\\/])global\.css$/i.test(filename);
}

function isCssFile(filename: string): boolean {
  return /\.css$/i.test(filename);
}

function lineCol(source: string, index: number): { line: number; column: number } {
  let line = 1;
  let lastNewline = -1;
  for (let i = 0; i < index && i < source.length; i++) {
    if (source.charCodeAt(i) === 10) {
      line++;
      lastNewline = i;
    }
  }
  return { line, column: index - lastNewline };
}

function finding(
  source: string,
  index: number,
  rule: string,
  severity: "error" | "warning",
  message: string,
): DesignLintFinding {
  const { line, column } = lineCol(source, index);
  return { rule, severity, line, column, message };
}

/** Parse `--name: value` custom properties out of a CSS source. */
export function extractCustomPropertyTokens(source: string): DesignTokenMap {
  const tokens: DesignTokenMap = {};
  const blanked = blankCssComments(source);
  for (const block of cssBlocks(blanked)) {
    if (isTokenBlock(block.body)) {
      for (const decl of blanked.slice(block.bodyStart, block.bodyEnd).matchAll(TOKEN_DECL_RE)) {
        tokens[decl[1] ?? ""] = (decl[2] ?? "").trim();
      }
    }
  }
  return tokens;
}

function blankCssComments(source: string): string {
  return source.replace(CSS_COMMENT_RE, (m) => " ".repeat(m.length));
}

interface CssBlock {
  selectorStart: number;
  bodyStart: number;
  bodyEnd: number;
  body: string;
}

/** Innermost `selector { body }` blocks, with absolute offsets. */
function cssBlocks(source: string): CssBlock[] {
  const blocks: CssBlock[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  for (const m of source.matchAll(re)) {
    const start = m.index ?? 0;
    blocks.push({
      selectorStart: start,
      bodyStart: start + m[0].indexOf("{") + 1,
      bodyEnd: start + m[0].length - 1,
      body: m[2] ?? "",
    });
  }
  return blocks;
}

function isTokenBlock(body: string): boolean {
  return /--[\w-]+\s*:/.test(body);
}

function classifyGradientStop(stop: string): string | null {
  const trimmed = stop.trim();
  if (trimmed.startsWith("#")) {
    const norm = normalizeValue(trimmed);
    for (const [family, hexes] of Object.entries(GRADIENT_FAMILIES)) {
      if (hexes.some((h) => normalizeValue(h) === norm)) return family;
    }
    return null;
  }
  const keyword = /\b(purple|violet|indigo|blue|cyan|pink|fuchsia|magenta)\b/i.exec(trimmed);
  return keyword ? (GRADIENT_KEYWORDS[keyword[1] ?? ""] ?? null) : null;
}

/** Scan a stretch of source for colour literals, naming a matching token. */
function scanColourLiterals(
  source: string,
  from: number,
  to: number,
  tokenByValue: DesignTokenMap,
  skipRanges: Array<[number, number]>,
  out: DesignLintFinding[],
): void {
  const skip = (index: number): boolean => skipRanges.some(([a, b]) => index >= a && index < b);
  const segment = source.slice(from, to);
  for (const m of segment.matchAll(HEX_RE)) {
    const abs = from + (m.index ?? 0);
    if (skip(abs) || !looksLikeColourValue(source, abs + (m[0] ?? "").length)) continue;
    const hex = (m[0] ?? "").toLowerCase();
    if (AI_INDIGO.has(hex)) continue; // more specific rule below
    const tokenName = tokenByValue[hex];
    const where = tokenName
      ? `${hex} is ${tokenName}; use var(${tokenName})`
      : "declare it as a custom property in src/styles/global.css and use var()";
    out.push(
      finding(
        source,
        abs,
        "raw-hex",
        "error",
        `Hardcoded colour ${hex} escapes theming — ${where}.`,
      ),
    );
  }
  for (const m of segment.matchAll(FUNC_COLOUR_RE)) {
    const abs = from + (m.index ?? 0);
    if (skip(abs)) continue;
    const open = source.indexOf("(", abs);
    const close = source.indexOf(")", abs);
    const rawInner = source.slice(open + 1, close);
    const inner = normalizeValue(rawInner);
    let tokenName = tokenByValue[inner];
    let alpha: string | null = null;
    if (!tokenName) {
      // rgba(200, 83, 43, 0.46) re-states --terracotta-rgb with alpha;
      // name the token instead of telling the author to invent one.
      for (const [value, name] of Object.entries(tokenByValue)) {
        if (inner.startsWith(`${value},`)) {
          tokenName = name;
          const parts = rawInner.split(",");
          alpha = parts.length >= 4 ? parts.slice(3).join(",").trim() : null;
          break;
        }
      }
    }
    const where = tokenName
      ? alpha
        ? `this is ${tokenName} with alpha ${alpha}; use rgba(var(${tokenName}), ${alpha})`
        : `this value is ${tokenName}; use var(${tokenName})`
      : "declare the value as a custom property in src/styles/global.css and use var()";
    out.push(
      finding(
        source,
        abs,
        "raw-hex",
        "error",
        `Hardcoded rgb/hsl colour escapes theming — ${where}.`,
      ),
    );
  }
}

/** Two-stop "trust" gradients: purple→blue, blue→cyan, indigo→pink.
 * Returns the ranges the flagged gradients cover so the overlapping
 * per-literal rules stay quiet. */
function scanTrustGradients(
  source: string,
  from: number,
  to: number,
  out: DesignLintFinding[],
  flagged: Array<[number, number]>,
): void {
  const segment = source.slice(from, to);
  for (const m of segment.matchAll(GRADIENT_RE)) {
    const abs = from + (m.index ?? 0);
    const stops = (m[1] ?? "")
      .split(",")
      .map((stop) => classifyGradientStop(stop))
      .filter((family): family is string => family !== null);
    const families = new Set(stops);
    const hit = FORBIDDEN_GRADIENT_PAIRS.find(([a, b]) => families.has(a) && families.has(b));
    if (!hit) continue;
    flagged.push([abs, abs + m[0].length]);
    out.push(
      finding(
        source,
        abs,
        "trust-gradient",
        "error",
        `Two-stop ${hit[0]}→${hit[1]} "trust" gradient — replace with a flat surface from the palette (or a single var() colour).`,
      ),
    );
  }
}

function scanSolidIndigo(
  source: string,
  from: number,
  to: number,
  skipRanges: Array<[number, number]>,
  out: DesignLintFinding[],
): void {
  const segment = source.slice(from, to).toLowerCase();
  for (const hex of AI_DEFAULT_INDIGO) {
    let at = segment.indexOf(hex);
    while (at !== -1) {
      const abs = from + at;
      if (!skipRanges.some(([a, b]) => abs >= a && abs < b)) {
        out.push(
          finding(
            source,
            abs,
            "ai-default-indigo",
            "error",
            `Default Tailwind indigo accent (${hex}) — the textbook unedited-LLM colour. Use a palette token (var(--terracotta) et al.) or encode a deliberate accent in global.css.`,
          ),
        );
      }
      at = segment.indexOf(hex, at + 1);
    }
  }
}

interface JsxElement {
  tag: string;
  className: string;
}

interface JsxTextNode {
  text: string;
  start: number;
  stack: JsxElement[];
}

/** Minimal JSX scanner: JSX text nodes with their enclosing-element stack,
 * plus attribute values (user-visible copy lives in both). */
function scanJsxTextAndAttributes(source: string): {
  texts: JsxTextNode[];
  attributeValues: Array<{ text: string; start: number }>;
} {
  const texts: JsxTextNode[] = [];
  const attributeValues: Array<{ text: string; start: number }> = [];
  const stack: JsxElement[] = [];
  const tagRe = /<(\/?)([a-zA-Z][\w.-]*)((?:"[^"]*"|'[^']*'|[^>"'])*)(\/?)>/g;
  let last = 0;
  for (const m of source.matchAll(tagRe)) {
    const between = source.slice(last, m.index ?? 0);
    if (/\S/.test(between)) texts.push({ text: between, start: last, stack: [...stack] });
    const isClose = m[1] === "/";
    const tag = (m[2] ?? "").toLowerCase();
    const attrs = m[3] ?? "";
    const attrsStart = (m.index ?? 0) + 1 + (isClose ? 1 : 0) + (m[2] ?? "").length;
    if (!isClose) {
      for (const am of attrs.matchAll(/([-\w:]+)\s*=\s*(?:"([^"]*)"|'([^']*)')/g)) {
        const value = am[2] ?? am[3] ?? "";
        attributeValues.push({
          text: value,
          start: attrsStart + (am.index ?? 0) + am[0].indexOf(value),
        });
      }
      const selfClosed = m[4] === "/" || /\s\/$/.test(attrs.trimEnd());
      const className = /className\s*=\s*(?:"([^"]*)"|'([^']*)')/.exec(attrs);
      if (!selfClosed && !VOID_TAGS.has(tag)) {
        stack.push({ tag, className: className?.[1] ?? className?.[2] ?? "" });
      }
    } else {
      for (let i = stack.length - 1; i >= 0; i--) {
        if (stack[i]?.tag === tag) {
          stack.length = i;
          break;
        }
      }
    }
    last = (m.index ?? 0) + m[0].length;
  }
  const tail = source.slice(last);
  if (/\S/.test(tail)) texts.push({ text: tail, start: last, stack: [...stack] });
  return { texts, attributeValues };
}

function isIconContext(stack: JsxElement[]): boolean {
  return stack.some(
    (el) => ICON_TAGS.has(el.tag) || (el.className && /\bicon\b/.test(el.className)),
  );
}

function scanEmojiIcons(source: string, texts: JsxTextNode[], out: DesignLintFinding[]): void {
  for (const node of texts) {
    if (!isIconContext(node.stack)) continue;
    for (const m of node.text.matchAll(EMOJI_RE)) {
      const emoji = m[0] ?? "";
      if (EMOJI_ALLOWLIST.has(emoji)) continue;
      out.push(
        finding(
          source,
          node.start + (m.index ?? 0),
          "emoji-icon",
          "warning",
          `Emoji "${emoji}" used as a UI icon — replace with a small monoline SVG (currentColor) or drop the icon.`,
        ),
      );
    }
  }
}

function scanPhrases(
  source: string,
  pieces: Array<{ text: string; start: number }>,
  rule: string,
  patterns: RegExp[],
  message: (match: string) => string,
  out: DesignLintFinding[],
): void {
  for (const piece of pieces) {
    for (const pattern of patterns) {
      const m = pattern.exec(piece.text);
      if (!m) continue;
      // One finding per rule per piece — a sentence that trips several
      // phrasings is one defect, not several.
      out.push(finding(source, piece.start + m.index, rule, "error", message(m[0] ?? "")));
      break;
    }
  }
}

/**
 * Lint one source file. `knownTokens` maps palette custom-property names
 * (--terracotta, …) to their values; report.ts loads them from
 * src/styles/global.css. When linting global.css itself the tokens are
 * read from the source, so the parameter is not needed.
 */
export function lintSource(
  source: string,
  filename: string,
  knownTokens?: DesignTokenMap,
): DesignLintFinding[] {
  const out: DesignLintFinding[] = [];
  const flaggedGradients: Array<[number, number]> = [];
  const tokenByValue: DesignTokenMap = {};
  for (const [name, value] of Object.entries(knownTokens ?? {})) {
    tokenByValue[normalizeValue(value)] = name;
  }

  if (isCssFile(filename)) {
    const blanked = blankCssComments(source);
    const exempt: Array<[number, number]> = [];
    for (const block of cssBlocks(blanked)) {
      if (isTokenBlock(block.body)) {
        exempt.push([block.bodyStart, block.bodyEnd]);
        if (isGlobalCss(filename)) {
          for (const decl of blanked
            .slice(block.bodyStart, block.bodyEnd)
            .matchAll(TOKEN_DECL_RE)) {
            tokenByValue[normalizeValue(decl[2] ?? "")] = decl[1] ?? "";
          }
        }
      }
    }
    for (const block of cssBlocks(blanked)) {
      if (exempt.some(([a, b]) => block.bodyStart >= a && block.bodyEnd <= b)) continue;
      scanTrustGradients(blanked, block.bodyStart, block.bodyEnd, out, flaggedGradients);
      scanColourLiterals(
        blanked,
        block.bodyStart,
        block.bodyEnd,
        tokenByValue,
        [...exempt, ...flaggedGradients],
        out,
      );
      scanSolidIndigo(
        blanked,
        block.bodyStart,
        block.bodyEnd,
        [...exempt, ...flaggedGradients],
        out,
      );
    }
    return sortFindings(out);
  }

  // TSX (or unknown text): scan the whole source for colours, JSX text
  // and attribute values for copy rules.
  scanTrustGradients(source, 0, source.length, out, flaggedGradients);
  const skip = [...flaggedGradients];
  scanColourLiterals(source, 0, source.length, tokenByValue, skip, out);
  scanSolidIndigo(source, 0, source.length, skip, out);
  const { texts, attributeValues } = scanJsxTextAndAttributes(source);
  scanEmojiIcons(source, texts, out);
  const visible: Array<{ text: string; start: number }> = [
    ...texts.map((t) => ({ text: t.text, start: t.start })),
    ...attributeValues,
  ];
  scanPhrases(
    source,
    visible,
    "filler-copy",
    FILLER_PATTERNS,
    (match) =>
      `Filler copy "${match}" — ship real, product-specific copy or leave the element out.`,
    out,
  );
  scanPhrases(
    source,
    visible,
    "invented-metric",
    METRIC_PATTERNS,
    (match) =>
      `Unsourced marketing claim "${match}" — use a real number with a source, or a labelled placeholder.`,
    out,
  );
  return sortFindings(out);
}

function sortFindings(out: DesignLintFinding[]): DesignLintFinding[] {
  return out.sort((a, b) => a.line - b.line || (a.column ?? 0) - (b.column ?? 0));
}
