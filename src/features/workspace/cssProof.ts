// Loading the real stylesheets for computed-style proof without launching
// the app: comments stripped, theme tokens resolved, and named rules injected
// in sheet order, so the cascade under test is the bundle's own.
//
// Selected rules are injected in their original sheet order, and the DOM's
// CSS engine resolves the real descendant, attribute, and specificity
// relationships between them. This is a computed-style proof for the selected
// DOM states, not a whole-app browser run. Happy DOM does not expose generated
// pseudo-element content reliably; use `rulesFor` for those declarations.

export type CssTheme = "light" | "dark";

interface CssRule {
  selector: string;
  body: string;
}

function parseRules(css: string): CssRule[] {
  const rules: CssRule[] = [];
  let index = 0;
  while (index < css.length) {
    const open = css.indexOf("{", index);
    if (open < 0) break;
    const selector = css.slice(index, open).trim();
    const close = css.indexOf("}", open);
    if (close < 0) break;
    const body = css.slice(open + 1, close);
    if (selector.startsWith("@")) {
      let depth = 1;
      let cursor = open + 1;
      while (depth > 0 && cursor < css.length) {
        if (css[cursor] === "{") depth += 1;
        if (css[cursor] === "}") depth -= 1;
        cursor += 1;
      }
      index = cursor;
      continue;
    }
    rules.push({ selector: selector.replace(/\s+/g, " "), body });
    index = close + 1;
  }
  return rules;
}

function selectorMatches(ruleSelector: string, target: string): boolean {
  return ruleSelector
    .split(",")
    .map((part) => part.trim())
    .some((part) => part === target);
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
  const allRules = parseRules(resolved);

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

  return { rulesFor, inject, token: (name: string) => tokens.get(name) };
}

/** Removes every `<style>` an `inject` above added. Call in `afterEach`. */
export function removeCssProof(): void {
  document.querySelectorAll("style[data-css-proof]").forEach((el) => el.remove());
}
