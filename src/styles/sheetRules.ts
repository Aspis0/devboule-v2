// The stylesheets as the walks read them: every rule with its declarations,
// the tokens of each theme, and a token resolved to the #rrggbb it paints.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { HEX_COLOR } from "./contrast";
import { collectTokens, parseRules, resolveVars, stripComments } from "./cssText";
import { collectSrcSheets } from "./srcSheets";

export const THEMES = ["light", "dark"] as const;
export type Theme = (typeof THEMES)[number];

const tokensCss = stripComments(readFileSync(resolve(import.meta.dirname, "tokens.css"), "utf8"));

export const tokens: Record<Theme, Map<string, string>> = {
  light: collectTokens(tokensCss, "light"),
  dark: collectTokens(tokensCss, "dark"),
};

export interface SheetRule {
  file: string;
  selector: string;
  declarations: Map<string, string>;
}

function declarationsOf(body: string): Map<string, string> {
  const declarations = new Map<string, string>();
  for (const declaration of body.split(";")) {
    const match = /^\s*([a-zA-Z-]+)\s*:\s*([\s\S]+?)\s*$/.exec(declaration);
    if (match !== null) {
      declarations.set(match[1]!.toLowerCase(), match[2]!.replace(/\s*!\s*important$/i, ""));
    }
  }
  return declarations;
}

/** Every rule of every shipped sheet except the token sheet itself. */
export const RULES: SheetRule[] = collectSrcSheets()
  .filter((sheet) => sheet.path !== "src/styles/tokens.css")
  .flatMap((sheet) =>
    parseRules(stripComments(sheet.css), { onNesting: "skip" }).map((rule) => ({
      file: sheet.path,
      selector: rule.selector,
      declarations: declarationsOf(rule.body),
    })),
  );

export const label = (rule: SheetRule): string => `${rule.file}: ${rule.selector}`;

export function find(file: string, selector: string): SheetRule | undefined {
  return RULES.find((rule) => rule.file === file && rule.selector === selector);
}

/** The token a declaration value names when it is exactly one var(). */
export function varName(value: string | undefined): string | null {
  const match = /^var\(\s*(--[a-zA-Z0-9-]+)\s*\)$/.exec(value?.trim() ?? "");
  return match === null ? null : match[1]!;
}

/** The ground token a rule paints, when it paints one token on its own. */
export function groundOf(rule: SheetRule): string | null {
  return varName(rule.declarations.get("background") ?? rule.declarations.get("background-color"));
}

/** A token's #rrggbb in one theme; throws when it does not resolve to one. */
export function hex(name: string, theme: Theme): string {
  const value = resolveVars(`var(${name})`, tokens[theme]).text.trim();
  if (!HEX_COLOR.test(value)) throw new Error(`${name} does not resolve to a hex colour: ${value}`);
  return value;
}
