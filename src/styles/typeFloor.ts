// The 12px type floor as a walk over the sheets' real text. Every font-size
// a sheet can paint — a literal, a token, an alias chain, a shorthand, or a
// math function, anywhere including inside at-rule blocks — must resolve to
// at least the floor, and a size the walk cannot resolve is a finding, never
// a skip. `inherit` is the one non-size: it paints the parent's computed
// size, and every declared size it can end up at is walked by the same run.
//
// This is a declaration linter: it certifies what the sheets say, not what
// a later rule paints over. Computed-style proofs live in cssProof.

import { collectTokens, parseRules, resolveVars, splitTopLevel, stripComments } from "./cssText";

export const TYPE_FLOOR_PX = 12;

/** The walk's substitute for a real root font-size: the UA default. */
const ROOT_PX = 16;

export interface TypeFloorFinding {
  rule: string;
  declaration: string;
  px: number | null;
  reason: string;
}

interface ResolvedRule {
  selector: string;
  body: string;
  stuck: ReadonlySet<string>;
}

interface Sheet {
  rules: ResolvedRule[];
  tokens: ReadonlyMap<string, string>;
}

interface Resolution {
  px: number | null;
  reason?: string;
}

interface EvalContext {
  sheet: Sheet;
  rule: ResolvedRule;
  seen: ReadonlySet<ResolvedRule>;
}

const LENGTH = /^(\d+(?:\.\d+)?)(px|rem|em|%)$/i;
const MATH_FN = /^(clamp|min|max)\((.*)\)$/is;

function classesOf(part: string): Set<string> {
  return new Set([...part.matchAll(/\.([a-zA-Z0-9_-]+)/g)].map((m) => m[1]!));
}

/** A bare-class rule governs the parent element when its classes are a
 * subset of the ancestor's: `.tool` matches the `.tool.is-plan` element. */
function coversAncestor(candidatePart: string, ancestor: string): boolean {
  const candidate = classesOf(candidatePart);
  if (candidate.size === 0) return false;
  const wanted = classesOf(ancestor);
  return [...candidate].every((name) => wanted.has(name));
}

function ancestorOfPart(part: string): string | null {
  const match = /^([\s\S]*)(?:[\s>+~])+([^\s>+~]+)$/.exec(part);
  if (!match) return null;
  const ancestor = match[1]!.trim();
  return ancestor.length > 0 ? ancestor : null;
}

/** The size a single declaration declares: its own `font-size`, or the size
 * inside a `font` shorthand. */
function declarationSize(decl: string): { expr: string | null; why?: string } {
  const direct = /^font-size\s*:\s*(.+)$/is.exec(decl);
  if (direct) return { expr: direct[1]!.trim() };
  const shorthand = /^font\s*:\s*(.+)$/is.exec(decl);
  if (!shorthand) return { expr: null };
  const front = (splitTopLevel(shorthand[1]!)[0] ?? "").split("/")[0]!.trim();
  const size = front.split(/\s+/).find((token) => /^[\d.]+[a-z%]*$/i.test(token));
  if (size) return { expr: size };
  if (/^inherit$/i.test(front)) return { expr: "inherit" };
  return { expr: null, why: "font shorthand without an extractable size" };
}

function evaluateSize(expr: string, ctx: EvalContext): Resolution {
  const value = expr.trim().replace(/\s*!\s*important$/i, "");
  if (/^inherit$/i.test(value)) return parentSize(ctx);
  const fn = MATH_FN.exec(value);
  if (fn) {
    const kind = fn[1]!.toLowerCase();
    const args = splitTopLevel(fn[2]!).map((arg) => evaluateSize(arg, ctx));
    // clamp's output never dips under its first argument; min() can land on
    // any argument; max() can only rise above its resolvable ones.
    if (kind === "clamp") return args[0]!;
    if (kind === "min") {
      const stuck = args.find((arg) => arg.px === null);
      if (stuck) return stuck;
      return { px: Math.min(...args.map((arg) => arg.px!)) };
    }
    const resolved = args.filter((arg) => arg.px !== null);
    if (resolved.length === 0) {
      return { px: null, reason: `max(${fn[2]}) has no resolvable lower bound` };
    }
    return { px: Math.max(...resolved.map((arg) => arg.px!)) };
  }
  const size = LENGTH.exec(value);
  if (!size) {
    return { px: null, reason: `"${value}" is not a size the walk can resolve` };
  }
  const amount = Number.parseFloat(size[1]!);
  switch (size[2]!.toLowerCase()) {
    case "px":
      return { px: amount };
    case "rem":
      return { px: amount * ROOT_PX };
    case "%":
      return { px: (amount * ROOT_PX) / 100 };
    default: {
      // em scales by the parent the same sheet proves; see parentSize.
      const parent = parentSize(ctx);
      if (parent.px === null) return parent;
      return { px: amount * parent.px };
    }
  }
}

/** The parent's size, read off the same sheet: for each selector part, the
 * smallest font-size any governing rule declares for that part's ancestor.
 * Smallest, because a floor check may not guess a parent larger than any
 * rule proves. */
function parentSize(ctx: EvalContext): Resolution {
  let best: number | null = null;
  let ancestor: string | null = null;
  for (const part of ctx.rule.selector.split(",")) {
    const candidate = ancestorOfPart(part.replace(/\s+/g, " ").trim());
    if (candidate === null) continue;
    ancestor ??= candidate;
    for (const rule of ctx.sheet.rules) {
      if (ctx.seen.has(rule)) continue;
      const size = rule.body
        .split(";")
        .map((declaration) => declarationSize(declaration.trim()).expr)
        .find((expr) => expr !== null);
      if (size === undefined) continue;
      if (
        !rule.selector
          .split(",")
          .some((p) => coversAncestor(p.replace(/\s+/g, " ").trim(), candidate))
      ) {
        continue;
      }
      const inner = evaluateSize(size, {
        sheet: ctx.sheet,
        rule,
        seen: new Set(ctx.seen).add(rule),
      });
      if (inner.px !== null && (best === null || inner.px < best)) best = inner.px;
    }
  }
  if (best === null) {
    return {
      px: null,
      reason: ancestor
        ? `no rule in the same sheet sets a font-size for the parent "${ancestor}"`
        : "the rule names no parent whose font-size the same sheet could declare",
    };
  }
  return { px: best };
}

function varReason(
  expr: string | null,
  stuck: ReadonlySet<string>,
  tokens: ReadonlyMap<string, string>,
): string | null {
  if (expr === null || !/var\(/i.test(expr)) return null;
  // The declaration's own stuck var(), not the rule's: a sibling
  // declaration's colour token must not be blamed for a size failure.
  const own = [...expr.matchAll(/var\(\s*(--[a-zA-Z0-9-]+)/gi)]
    .map((match) => match[1]!)
    .find((name) => stuck.has(name));
  if (own === undefined) return "a var() the walk could not resolve";
  return tokens.has(own)
    ? `custom property ${own} sits on a cyclic alias chain`
    : `unknown custom property ${own}`;
}

export function findBelowTypeFloor(
  sheets: readonly string[],
  theme: "light" | "dark" = "light",
): TypeFloorFinding[] {
  const findings: TypeFloorFinding[] = [];
  // Tokens are a property of the bundle: tokens.css is its own sheet, so the
  // maps merge in sheet order before any body is resolved — per theme, so a
  // size the dark ramp drops is never judged at the light ramp's value. The
  // parent of an em is still read sheet-locally, per parentSize.
  const tokens = new Map<string, string>();
  const parsed = sheets.map((sheetText) => {
    const css = stripComments(sheetText);
    const rules = parseRules(css);
    for (const [name, value] of collectTokens(css, theme)) tokens.set(name, value);
    return rules;
  });
  const resolved = parsed.map((rules) =>
    rules.map((rule) => {
      const { text, stuck } = resolveVars(rule.body, tokens);
      return { selector: rule.selector, body: text, stuck };
    }),
  );
  for (const rules of resolved) {
    const sheet: Sheet = { rules, tokens };
    for (const rule of sheet.rules) {
      for (const declaration of rule.body.split(";")) {
        const decl = declaration.trim();
        const { expr, why } = declarationSize(decl);
        if (expr === null && why === undefined) continue;
        // See the header: inherit adds no size of its own.
        if (expr !== null && /^inherit$/i.test(expr)) continue;
        const resolution =
          expr === null
            ? { px: null as number | null, reason: why }
            : evaluateSize(expr, { sheet, rule, seen: new Set<ResolvedRule>([rule]) });
        if (resolution.px !== null && resolution.px >= TYPE_FLOOR_PX) continue;
        findings.push({
          rule: rule.selector,
          declaration: decl,
          px: resolution.px,
          reason:
            resolution.px === null
              ? (varReason(expr, rule.stuck, sheet.tokens) ?? resolution.reason ?? "unresolvable")
              : `${resolution.px}px is below the ${TYPE_FLOOR_PX}px floor`,
        });
      }
    }
  }
  return findings;
}
