// Loading the real stylesheets for computed-style proof without launching
// the app: comments stripped, theme tokens resolved, and named rules injected
// in sheet order, so the cascade under test is the bundle's own.
//
// Selected rules are injected in their original sheet order, and the DOM's
// CSS engine resolves the real descendant, attribute, and specificity
// relationships between them. This is a computed-style proof for the selected
// DOM states, not a whole-app browser run. Happy DOM does not expose generated
// pseudo-element content reliably; use `rulesFor` for those declarations.

import { parseRules, splitTopLevel } from "../../styles/cssText";

export type CssTheme = "light" | "dark";

export interface CssRule {
  selector: string;
  body: string;
}

export function selectorMatches(ruleSelector: string, target: string): boolean {
  return ruleSelector
    .split(",")
    .map((part) => part.trim())
    .some((part) => part === target);
}

/** A font-family declaration naming the mono stack, directly or by token. */
const MONO_FAMILY = /font-family:[^;]*(?:"JetBrains Mono"|var\(--font-mono\))/;

function isIdentChar(char: string): boolean {
  return /[\w-]/.test(char);
}

function strongerSpec(
  left: readonly [number, number, number],
  right: readonly [number, number, number],
): boolean {
  return (
    left[0] > right[0] ||
    (left[0] === right[0] && (left[1] > right[1] || (left[1] === right[1] && left[2] > right[2])))
  );
}

function specificityOfPart(part: string): [number, number, number] {
  let ids = 0;
  let classes = 0;
  let types = 0;
  let i = 0;
  while (i < part.length) {
    const char = part[i];
    if (char === "#" || char === ".") {
      // The name that follows is part of the same simple selector.
      i += 1;
      while (i < part.length && isIdentChar(part[i])) i += 1;
      if (char === "#") ids += 1;
      else classes += 1;
    } else if (char === "[") {
      classes += 1;
      const close = part.indexOf("]", i + 1);
      i = close < 0 ? part.length : close + 1;
    } else if (char === ":") {
      if (part[i + 1] === ":") {
        types += 1;
        i += 2;
        while (i < part.length && isIdentChar(part[i])) i += 1;
      } else {
        let j = i + 1;
        while (j < part.length && isIdentChar(part[j])) j += 1;
        const name = part.slice(i + 1, j);
        if (part[j] === "(") {
          let depth = 1;
          let k = j + 1;
          while (k < part.length && depth > 0) {
            if (part[k] === "(") depth += 1;
            else if (part[k] === ")") depth -= 1;
            k += 1;
          }
          const argument = part.slice(j + 1, k - 1);
          if (name === "has" || name === "is" || name === "not") {
            const argSpec = specificity(argument);
            ids += argSpec[0];
            classes += argSpec[1];
            types += argSpec[2];
          } else if (name !== "where") {
            classes += 1;
          }
          i = k;
        } else {
          classes += 1;
          i = j;
        }
      }
    } else if (isIdentChar(char)) {
      types += 1;
      while (i < part.length && isIdentChar(part[i])) i += 1;
    } else {
      // Combinators and the universal selector carry no specificity.
      i += 1;
    }
  }
  return [ids, classes, types];
}

/** Specificity (a, b, c) of a selector list — its most specific part — per
 * CSS Selectors 4: :has() and :is() contribute their most specific argument,
 * :not() its own, :where() nothing. */
export function specificity(selector: string): [number, number, number] {
  let best: [number, number, number] = [0, 0, 0];
  for (const part of splitTopLevel(selector)) {
    const spec = specificityOfPart(part);
    if (strongerSpec(spec, best)) best = spec;
  }
  return best;
}

function darkThemeBodies(css: string): string[] {
  const bodies: string[] = [];
  const headers = /\[data-theme=["']dark["']\]\s*\{/g;
  for (let match = headers.exec(css); match !== null; match = headers.exec(css)) {
    const open = css.indexOf("{", match.index);
    let depth = 1;
    let cursor = open + 1;
    while (depth > 0 && cursor < css.length) {
      if (css[cursor] === "{") depth += 1;
      if (css[cursor] === "}") depth -= 1;
      cursor += 1;
    }
    if (depth === 0) bodies.push(css.slice(open + 1, cursor - 1));
    headers.lastIndex = cursor;
  }
  return bodies;
}

/** The assembled sheets, in bundle order: token resolution, rule lookup,
 * and injection all read this one joined source. */
export function assembleCssProof(
  sheets: readonly string[],
  theme: CssTheme = "light",
): {
  rulesFor: (target: string) => string;
  inject: (targets: readonly string[]) => void;
  /** The selected theme's token value, read from the sheets themselves. */
  token: (name: string) => string | undefined;
  /** Every assembled rule that declares a mono font-family. */
  monoDeclarations: { selector: string; body: string }[];
  /** Every parsed rule of the assembled sheet, in sheet order. */
  rules: readonly CssRule[];
} {
  if (theme !== "light" && theme !== "dark") {
    throw new Error(`Unsupported CSS proof theme: ${String(theme)}`);
  }
  const stripped = sheets.map((sheet) => sheet.replace(/\/\*[\s\S]*?\*\//g, ""));
  const tokens = new Map<string, string>();
  for (const sheet of stripped) {
    for (const block of sheet.matchAll(/:root\s*\{([^}]*)\}/g)) {
      for (const m of block[1]!.matchAll(/--([a-zA-Z0-9-]+):\s*([^;]+);/g)) {
        tokens.set(`--${m[1]!.trim()}`, m[2]!.trim());
      }
    }
  }
  if (theme === "dark") {
    for (const sheet of stripped) {
      for (const body of darkThemeBodies(sheet)) {
        for (const m of body.matchAll(/--([a-zA-Z0-9-]+):\s*([^;]+);/g)) {
          tokens.set(`--${m[1]!.trim()}`, m[2]!.trim());
        }
      }
    }
  }
  let resolved = stripped.join("\n");
  for (let pass = 0; pass < 4; pass += 1) {
    resolved = resolved.replace(
      /var\((--[a-zA-Z0-9-]+)\)/g,
      (whole: string, name: string) => tokens.get(name) ?? whole,
    );
  }
  const allRules = parseRules(resolved, { onNesting: "skip" });

  function rulesFor(target: string): string {
    return allRules
      .filter((rule) => selectorMatches(rule.selector, target))
      .map((rule) => rule.body)
      .join("\n");
  }

  function inject(targets: readonly string[]): void {
    const picked = allRules.filter((rule) =>
      targets.some((target) => selectorMatches(rule.selector, target)),
    );
    const style = document.createElement("style");
    style.setAttribute("data-css-proof", "");
    style.textContent = picked.map((rule) => `${rule.selector} { ${rule.body} }`).join("\n");
    document.head.appendChild(style);
  }

  return {
    rulesFor,
    inject,
    token: (name: string) => tokens.get(name),
    /** Every assembled rule that declares a mono font-family — the walking
     * test's raw material. The policy (which selectors may be mono) lives in
     * the test; the parsing lives here, so a walk never depends on a
     * hand-written target list. */
    monoDeclarations: allRules.filter((rule) => MONO_FAMILY.test(rule.body)),
    rules: allRules,
  };
}

/** Removes every `<style>` an `inject` above added. Call in `afterEach`. */
export function removeCssProof(): void {
  document.querySelectorAll("style[data-css-proof]").forEach((el) => el.remove());
}
