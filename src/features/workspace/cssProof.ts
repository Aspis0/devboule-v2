// Loading the real stylesheets for computed-style proof without launching
// the app: comments stripped, tokens resolved to their light values, and the
// rules a test names injected into the document in sheet order, so the
// cascade under test is the bundle's own.

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

/** The assembled sheets, in bundle order: token resolution, rule lookup,
 * and injection all read this one joined source. */
export function assembleCssProof(sheets: readonly string[]): {
  rulesFor: (target: string) => string;
  inject: (targets: readonly string[]) => void;
} {
  const stripped = sheets.map((sheet) => sheet.replace(/\/\*[\s\S]*?\*\//g, ""));
  const tokens = new Map<string, string>();
  for (const sheet of stripped) {
    for (const block of sheet.matchAll(/:root\s*\{([^}]*)\}/g)) {
      for (const m of block[1]!.matchAll(/--([a-zA-Z0-9-]+):\s*([^;]+);/g)) {
        tokens.set(`--${m[1]!.trim()}`, m[2]!.trim());
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

  return { rulesFor, inject };
}

/** Removes every `<style>` an `inject` above added. Call in `afterEach`. */
export function removeCssProof(): void {
  document.querySelectorAll("style[data-css-proof]").forEach((el) => el.remove());
}
