// Accent text rides --accent-text. A text-colour declaration — `color` or
// `-webkit-text-fill-color` — may not name the fill accent or any custom
// property that resolves onto it. The alias set is derived from tokens.css
// (every token whose value is a var() chain ending at --accent), never a
// hand list. Each declaration's value is judged as written: !important
// stripped, every var() reference checked at any case or spacing, including
// inside fallback chains and color-mix() operands. Fills, borders, outlines
// and shadows are not policed. The one allow-listed exception paints the
// surface's initial letter; a contrast pair on its own ground in
// palette-contrast.test.ts holds it to 4.5:1.
// @vitest-environment node
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { collectTokens, parseRules, resolveVars, stripComments } from "./cssText";
import { collectSrcSheets, type SrcSheet } from "./srcSheets";

const TEXT_COLOR = /^(color|-webkit-text-fill-color)$/i;
const VAR_REF = /var\(\s*--([a-zA-Z0-9-]+)/gi;

const tokensCss = stripComments(readFileSync(resolve(import.meta.dirname, "tokens.css"), "utf8"));

/** Every custom property whose value resolves onto the fill accent. Tokens
 * are resolved with --accent removed from the map, so an alias stays stuck
 * at exactly var(--accent) while a tint like --accent-soft (a color-mix,
 * not a pure hop) stays outside the family. */
function accentFamily(css: string): Set<string> {
  const family = new Set<string>(["--accent"]);
  for (const theme of ["light", "dark"] as const) {
    const tokens = collectTokens(css, theme);
    const withoutAccent = new Map(tokens);
    withoutAccent.delete("--accent");
    for (const [name, value] of tokens) {
      if (resolveVars(value, withoutAccent).text.trim() === "var(--accent)") family.add(name);
    }
  }
  return family;
}

const FAMILY = accentFamily(tokensCss);

interface Exception {
  file: string;
  selector: string;
  why: string;
}

const EXCEPTIONS: readonly Exception[] = [
  {
    file: "src/styles/global.css",
    selector: ".nav-point:hover .nav-point-circle, .nav-point:focus-visible .nav-point-circle",
    why: "the circle paints the surface's initial letter — text; it stays on the fill accent because that clears 4.5:1 on its own card ground (4.747 light / 5.137 dark), guarded as a contrast pair in palette-contrast.test.ts",
  },
];

const allowList = new Set(EXCEPTIONS.map((e) => `${e.file}\n${e.selector}`));

function findAccentText(sheets: readonly SrcSheet[]): string[] {
  const problems: string[] = [];
  for (const sheet of sheets) {
    for (const rule of parseRules(stripComments(sheet.css), { onNesting: "skip" })) {
      for (const declaration of rule.body.split(";")) {
        const match = /^\s*(.+?)\s*:\s*([\s\S]+?)\s*$/is.exec(declaration);
        if (match === null || !TEXT_COLOR.test(match[1]!)) continue;
        const value = match[2]!.replace(/\s*!\s*important$/i, "").trim();
        const refs = [...value.matchAll(VAR_REF)].map((m) => `--${m[1]!}`);
        if (!refs.some((ref) => FAMILY.has(ref))) continue;
        if (allowList.has(`${sheet.path}\n${rule.selector}`)) continue;
        problems.push(`${sheet.path}: ${rule.selector} { ${declaration.trim()} }`);
      }
    }
  }
  return problems;
}

describe("accent text rides --accent-text", () => {
  it("finds no text colour on the fill accent outside the allow-list", () => {
    const sheets = collectSrcSheets();
    expect(sheets.length).toBeGreaterThan(0);
    expect(findAccentText(sheets)).toEqual([]);
  });

  it("holds the allow-list to rules that still exist, so it cannot rot", () => {
    const sheets = new Map(
      collectSrcSheets().map((sheet) => [sheet.path, stripComments(sheet.css)]),
    );
    const stale = EXCEPTIONS.filter((exception) => {
      const css = sheets.get(exception.file);
      return (
        css === undefined ||
        !parseRules(css, { onNesting: "skip" }).some((rule) => rule.selector === exception.selector)
      );
    });
    expect(stale.map((e) => `${e.file}: ${e.selector}`)).toEqual([]);
  });

  it("derives the accent family from tokens.css, not a hand list", () => {
    expect([...FAMILY].sort()).toEqual([
      "--accent",
      "--terracotta",
      "--terracotta-deep",
      "--terracotta-pressed",
    ]);
  });
});

function sheet(path: string, css: string): SrcSheet {
  return { path, css };
}

describe("the accent walk's contract", () => {
  const evasions: ReadonlyArray<[string, string]> = [
    ["spacing inside var()", ".p { color: var( --accent ); }"],
    ["a fallback chain reaching the accent", ".p { color: var(--nope, var(--accent)); }"],
    ["!important", ".p { color: var(--accent) !important; }"],
    ["a color-mix() operand", ".p { color: color-mix(in srgb, var(--accent) 30%, var(--ink)); }"],
    ["-webkit-text-fill-color", ".p { -webkit-text-fill-color: var(--accent); }"],
    ["the pressed alias", ".p { color: var(--terracotta-pressed); }"],
    ["uppercase COLOR", ".p { COLOR: var(--accent); }"],
    ["a rule inside @media", "@media (min-width: 10px) { .p { color: var(--accent); } }"],
    ["a multi-line declaration", ".p {\n  color:\n    var(--accent);\n}"],
  ];
  for (const [name, css] of evasions) {
    it(`fails on ${name}`, () => {
      expect(findAccentText([sheet("probe.css", css)])).toHaveLength(1);
    });
  }

  it("does not crash on a nested rule and still reports the flat finding", () => {
    const findings = findAccentText([
      sheet(
        "probe.css",
        ".flat { color: var(--accent); }\n.parent { color: var(--ink); &:hover { color: var(--accent); } }",
      ),
    ]);
    expect(findings).toHaveLength(1);
  });
});
