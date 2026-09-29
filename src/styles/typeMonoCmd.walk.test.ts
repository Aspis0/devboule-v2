// The command size (12.5px) exists once, as --type-mono-cmd in tokens.css.
// The walk judges the computed size, not the spelling: a font-size — or the
// size heading a font shorthand — is resolved through the tokens and fails
// when it lands on 12.5px, whether written 12.5px or 12.50px in any case,
// as a rem at the 16px root, or as a calc() of resolved lengths summed.
// em, %, clamp()/min()/max() and calc() products stay unproven and so
// unjudged, and so does TSX inline styling: the walk reads the sheets.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { collectTokens, parseRules, resolveVars, splitTopLevel, stripComments } from "./cssText";
import { collectSrcSheets, type SrcSheet } from "./srcSheets";

const COMMAND_PX = 12.5;
const ROOT_PX = 16;
const EPSILON = 1e-6;
const FONT = /^(font-size|font)\s*:\s*([\s\S]+)$/i;
const TOKEN_USE = /var\(\s*--type-mono-cmd\s*\)/i;

/** The size expression a declaration carries: its font-size, or the length
 * heading a font shorthand (a unitless weight there is not a size). */
function declaredSize(declaration: string): string | null {
  const match = FONT.exec(declaration.trim());
  if (match === null) return null;
  if (match[1]!.toLowerCase() === "font-size") return match[2]!.trim();
  const head = (splitTopLevel(match[2]!)[0] ?? "").split("/")[0] ?? "";
  return head.split(/\s+/).find((token) => /^\d[\d.]*(px|rem|em|%)$/i.test(token)) ?? null;
}

/** The px the expression computes to, where this walk can prove it: a bare
 * px or rem length (the rem at the 16px root), or a calc() of such lengths
 * summed. Anything else returns null and is not judged. */
function provenPx(expr: string, tokens: ReadonlyMap<string, string>): number | null {
  const { text, stuck } = resolveVars(expr, tokens);
  if (stuck.size > 0) return null;
  const value = text.trim();
  const calc = /^calc\((.*)\)$/is.exec(value);
  const inner = (calc === null ? value : calc[1]!).trim();
  if (/[()]/.test(inner)) return null;
  let total = 0;
  let consumed = 0;
  const term = /([+-]?)\s*(\d+(?:\.\d+)?)(px|rem)\s*/gi;
  for (let match = term.exec(inner); match !== null; match = term.exec(inner)) {
    if (match.index !== consumed) return null;
    const sign = match[1] === "-" ? -1 : 1;
    total +=
      sign * Number.parseFloat(match[2]!) * (match[3]!.toLowerCase() === "rem" ? ROOT_PX : 1);
    consumed = match.index + match[0].length;
  }
  return consumed === inner.length ? total : null;
}

function findCommandSizedText(sheets: readonly SrcSheet[]): string[] {
  const tokens = new Map<string, string>();
  const parsed = sheets.map((sheet) => {
    const css = stripComments(sheet.css);
    for (const [name, value] of collectTokens(css, "light")) tokens.set(name, value);
    return { path: sheet.path, rules: parseRules(css, { onNesting: "skip" }) };
  });
  const problems: string[] = [];
  for (const { path, rules } of parsed) {
    for (const rule of rules) {
      for (const declaration of rule.body.split(";")) {
        const expr = declaredSize(declaration);
        if (expr === null) continue;
        // Naming the token is the sanctioned way to be 12.5px; only a size
        // that lands on 12.5 without it is off the ramp.
        if (TOKEN_USE.test(expr)) continue;
        const px = provenPx(expr, tokens);
        if (px === null || Math.abs(px - COMMAND_PX) >= EPSILON) continue;
        problems.push(`${path}: ${rule.selector} { ${declaration.trim()} }`);
      }
    }
  }
  return problems;
}

describe("the command size lives in --type-mono-cmd", () => {
  it("finds no font-size that computes to 12.5px in any sheet", () => {
    const sheets = collectSrcSheets();
    expect(sheets.length).toBeGreaterThan(0);
    expect(findCommandSizedText(sheets)).toEqual([]);
  });
});

function sheet(path: string, css: string): SrcSheet {
  return { path, css };
}

describe("the command-size walk's contract", () => {
  const RAMP = ":root { --type-meta: 12px; --type-mono-cmd: 12.5px; }";
  const evasions: ReadonlyArray<[string, string]> = [
    ["a trailing zero", "font-size: 12.50px;"],
    ["rem at the 16px root", "font-size: 0.78125rem;"],
    ["an uppercase unit", "font-size: 12.5PX;"],
    ["a calc() of a token and px", "font-size: calc(var(--type-meta) + 0.5px);"],
  ];
  for (const [name, css] of evasions) {
    it(`fails on ${name}`, () => {
      expect(
        findCommandSizedText([sheet("ramp.css", RAMP), sheet("probe.css", `.p { ${css} }`)]),
      ).toHaveLength(1);
    });
  }

  it("fails the size inside a font shorthand", () => {
    expect(
      findCommandSizedText([sheet("probe.css", ".p { font: 12.5px/1.5 var(--font-mono); }")]),
    ).toHaveLength(1);
  });

  it("keeps the token's own use clean", () => {
    expect(
      findCommandSizedText([sheet("probe.css", ".p { font-size: var(--type-mono-cmd); }")]),
    ).toEqual([]);
  });

  it("leaves a calc() product unjudged rather than guess", () => {
    expect(
      findCommandSizedText([
        sheet("ramp.css", RAMP),
        sheet("probe.css", ".p { font-size: calc(25px * 0.5); }"),
      ]),
    ).toEqual([]);
  });

  it("does not crash on a nested rule and still reports the flat finding", () => {
    const findings = findCommandSizedText([
      sheet(
        "probe.css",
        ".flat { font-size: 12.50px; }\n.parent { font-size: 13px; &:hover { font-size: 12.5px; } }",
      ),
    ]);
    expect(findings).toEqual([expect.stringContaining(".flat { font-size: 12.50px")]);
  });
});
