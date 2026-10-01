import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * The band's guard reads the stylesheets, not the DOM: no layout runs in these
 * tests, so a getBoundingClientRect would be a zero rectangle for both boxes
 * and the overlap would pass unmeasured.
 *
 * What it keeps apart is the defect's own pair — the sliver's hit box and the
 * hint curve it overlaps against the page layer every surface is laid out in.
 * The sliver draws no ink of its own; the hint curve is the only closed-state
 * mark. Every surface root is a flex child that clips to its own box (`overflow: hidden`
 * on `.surface-card` and `.workspace-screen`), so the page layer's top edge is
 * the highest pixel the tab strip, or anything else inside a surface, can
 * paint: it is measured here, and Workspace.css is not read — another branch
 * rewrites it.
 */
const SHEET = readFileSync(new URL("../styles/global.css", import.meta.url), "utf8");
const TOKENS = readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8");

/** One rule's body, matched on its selector alone; comments are stripped first so a comment cannot carry the match. */
function rule(sheet: string, selector: string): string {
  const source = sheet.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of source.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    if (match[1].trim() === selector) return match[2];
  }
  throw new Error(`no rule for ${selector}`);
}

function declaration(declarations: string, property: string): string | undefined {
  return new RegExp(`^\\s*${property}:\\s*([^;]+);`, "m").exec(declarations)?.[1]?.trim();
}

/** A declaration's pixel length, var() (and its fallback, if one is written) resolved through the token sheet. */
function length(declarations: string, property: string, tokens: string): number | undefined {
  const declared = declaration(declarations, property);
  if (declared === undefined) return undefined;
  const value = declared.replace(
    /var\(\s*(--[\w-]+)\s*(?:,\s*([^)]+))?\s*\)/g,
    (_match, name: string, fallback?: string) => {
      const token = new RegExp(`${name}:\\s*([^;]+);`).exec(tokens)?.[1]?.trim();
      if (token !== undefined) return token;
      if (fallback !== undefined) return fallback;
      throw new Error(`undefined token ${name}`);
    },
  );
  if (value === "0") return 0;
  const px = /^(-?\d+(?:\.\d+)?)px$/.exec(value);
  return px === null ? undefined : Number(px[1]);
}

/** The same, for a length the guard cannot do without: unmeasurable fails the guard rather than vanishing from it. */
function pixels(declarations: string, property: string, tokens: string): number {
  const value = length(declarations, property, tokens);
  if (value === undefined) throw new Error(`cannot measure ${property} in pixels`);
  return value;
}

interface Band {
  /** The band's bottom edge, measured from the window's top. */
  bottom: number;
  sliverTop: number;
  sliverBottom: number;
  /** The hint curve's box bottom: the cue's region must reach through the band. */
  hintBottom: number;
  surfaceTop: number;
}

/**
 * Where the band's edges and the boxes sit. The band is the shell's own top
 * padding; the page layer hangs below it in flow, an absolute or fixed one
 * answers to the shell's padding box — the window's top edge — and so ignores
 * the band entirely, and a relative one shifts by its own `top`.
 */
function band(sheet: string, tokens: string): Band {
  const shellTop = pixels(rule(sheet, ".crescent-shell"), "top", tokens);
  const sliverDeclarations = rule(sheet, ".crescent-sliver");
  const hintDeclarations = rule(sheet, ".crescent-hint");
  const sliverTop = shellTop + pixels(sliverDeclarations, "top", tokens);
  const hintTop = shellTop + pixels(hintDeclarations, "top", tokens);

  const pageLayer = rule(sheet, ".page-layer");
  const bottom = length(rule(sheet, ".app-shell"), "padding-top", tokens) ?? 0;
  const position = declaration(pageLayer, "position") ?? "static";
  const inFlow = bottom + (length(pageLayer, "margin-top", tokens) ?? 0);
  const top = length(pageLayer, "top", tokens);
  const surfaceTop =
    (position === "absolute" || position === "fixed") && top !== undefined
      ? top
      : inFlow + (position === "relative" ? (top ?? 0) : 0);

  return {
    bottom,
    sliverTop,
    sliverBottom: sliverTop + pixels(sliverDeclarations, "height", tokens),
    hintBottom: hintTop + pixels(hintDeclarations, "height", tokens),
    surfaceTop,
  };
}

function violations(edges: Band): string[] {
  const problems: string[] = [];
  if (edges.bottom <= 0) {
    problems.push(`the band is ${edges.bottom}px, leaving no room for the trigger`);
  }
  if (edges.sliverTop < 0) {
    problems.push(`the sliver starts at ${edges.sliverTop}px, above the window's edge`);
  }
  if (edges.sliverBottom > edges.bottom) {
    problems.push(`the sliver ends at ${edges.sliverBottom}px, past the band's ${edges.bottom}px`);
  }
  if (edges.hintBottom < edges.bottom) {
    problems.push(`the hint ends at ${edges.hintBottom}px, short of the band's ${edges.bottom}px`);
  }
  if (edges.surfaceTop < edges.bottom) {
    problems.push(
      `the surfaces start at ${edges.surfaceTop}px, above the band's ${edges.bottom}px`,
    );
  }
  return problems;
}

interface Mutation {
  name: string;
  sheet: "shell" | "tokens";
  from: string;
  to: string;
}

/**
 * One deliberate defect each, named as it would show on the window. Each is
 * applied to the sheets in memory — the stylesheets on disk are never written.
 */
const MUTATIONS: Mutation[] = [
  {
    name: "the band is 0",
    sheet: "tokens",
    from: "--crescent-band: 13px;",
    to: "--crescent-band: 0px;",
  },
  {
    name: "the sliver is taller than the band",
    sheet: "shell",
    from: "height: var(--crescent-band);",
    to: "height: 20px;",
  },
  {
    name: "the page layer is absolute at the window's top",
    sheet: "shell",
    from: ".page-layer {\n  position: relative;",
    to: ".page-layer {\n  position: absolute;\n  top: 0;",
  },
  {
    name: "the page layer is pulled up into the band",
    sheet: "shell",
    from: ".page-layer {\n  position: relative;",
    to: ".page-layer {\n  margin-top: -13px;\n  position: relative;",
  },
  {
    name: "the hint is lifted clear of the band",
    sheet: "shell",
    from: "\n  top: -26px;\n",
    to: "\n  top: -40px;\n",
  },
];

function apply(mutation: Mutation, sheet: string, tokens: string): [string, string] {
  const target = mutation.sheet === "shell" ? sheet : tokens;
  const hits = target.split(mutation.from).length - 1;
  if (hits !== 1) {
    throw new Error(`${mutation.name}: the anchor was found ${hits} times, expected once`);
  }
  const replaced = target.replace(mutation.from, mutation.to);
  return mutation.sheet === "shell" ? [replaced, tokens] : [sheet, replaced];
}

/** Paint declarations: anything here on the trigger would be a second closed-state mark. Outline is absent on purpose — the keyboard focus ring. */
const SLIVER_PAINT = new Set([
  "background",
  "background-color",
  "border",
  "border-color",
  "border-top-color",
  "border-right-color",
  "border-bottom-color",
  "border-left-color",
  "box-shadow",
  "stroke",
  "fill",
]);

/**
 * Paint on the trigger, as `selector → property: value` lines: the bare button, its blocked
 * state, and every pseudo-element it could grow. Bare `transparent`/`none`/`0` values paint
 * nothing and stay silent. Single-colon interaction states are out of scope: the claim is the
 * closed band at rest.
 */
function sliverInk(sheet: string): string[] {
  const found: string[] = [];
  const source = sheet.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const match of source.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = match[1].split(",").map((part) => part.trim());
    const trigger = selectors.some(
      (selector) =>
        selector === ".crescent-sliver" ||
        selector === ".crescent-sliver-blocked" ||
        selector.startsWith(".crescent-sliver::"),
    );
    if (!trigger) continue;
    for (const declarationText of match[2].split(";")) {
      const colon = declarationText.indexOf(":");
      if (colon === -1) continue;
      const property = declarationText.slice(0, colon).trim();
      const value = declarationText.slice(colon + 1).trim();
      if (!SLIVER_PAINT.has(property)) continue;
      if (value === "transparent" || value === "none" || value === "0") continue;
      found.push(`${selectors.join(", ")} → ${property}: ${value}`);
    }
  }
  return found;
}

describe("the band that holds the crescent's sliver", () => {
  it("keeps the sliver inside it, the hint reaching through it, every surface below", () => {
    expect(violations(band(SHEET, TOKENS))).toEqual([]);
  });

  it("draws exactly one closed-state mark: the hint curve, never the sliver", () => {
    expect(sliverInk(SHEET)).toEqual([]);
    const before = `${SHEET}.crescent-sliver::before{content:"";background:red;}`;
    const after = `${SHEET}.crescent-sliver::after{content:"";background:red;}`;
    const silent = `${SHEET}.crescent-sliver::after{content:"";}`;
    expect(sliverInk(before)).not.toEqual([]);
    expect(sliverInk(after)).not.toEqual([]);
    expect(sliverInk(silent)).toEqual([]);
    const hint = rule(SHEET, ".crescent-hint");
    expect(declaration(hint, "opacity")).not.toBe("0");
    expect(rule(SHEET, ".crescent-hint path")).toContain("stroke: var(--muted)");
    expect(declaration(rule(SHEET, ".crescent-hint-hidden"), "opacity")).toBe("0");
  });

  for (const mutation of MUTATIONS) {
    it(`rejects a stylesheet where ${mutation.name}`, () => {
      const [sheet, tokens] = apply(mutation, SHEET, TOKENS);
      expect(violations(band(sheet, tokens))).not.toEqual([]);
    });
  }
});
