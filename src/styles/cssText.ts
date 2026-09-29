// Reading CSS text for the style walks: the rules (descending into every
// at-rule that can hold painted text), the custom properties each theme
// defines, and every var() resolved to its token's value — or left in place,
// named in `stuck`, when the chain is unknown or cyclic.

export interface ParsedRule {
  selector: string;
  body: string;
}

export function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, "");
}

function blockEnd(css: string, open: number): number {
  let depth = 1;
  let cursor = open + 1;
  while (depth > 0 && cursor < css.length) {
    if (css[cursor] === "{") depth += 1;
    else if (css[cursor] === "}") depth -= 1;
    cursor += 1;
  }
  return depth === 0 ? cursor - 1 : css.length;
}

/** At-rules whose bodies hold no painted text: keyframe steps, font
 * metadata, property descriptors. Everything else descends. */
const OPAQUE_AT_RULE = /^@(keyframes|font-face|property|counter-style)\b/i;

/** Rules at every level: statements are skipped, at-rules descend, and a
 * rule body that itself holds a block is native CSS nesting, which the walk
 * cannot read — it fails loudly rather than drop the nested rule. Callers
 * that cannot use a nested rule pass onNesting "skip" to drop it instead. */
export function parseRules(css: string, options?: { onNesting?: "throw" | "skip" }): ParsedRule[] {
  const rules: ParsedRule[] = [];
  let index = 0;
  while (index < css.length) {
    const open = css.indexOf("{", index);
    if (open < 0) break;
    const statement = css.indexOf(";", index);
    if (statement >= 0 && statement < open) {
      // A statement (@import, @charset) carries no block; skip past it so
      // the next rule's selector is not glued onto it and lost.
      index = statement + 1;
      continue;
    }
    const selector = css.slice(index, open).trim();
    const close = blockEnd(css, open);
    const body = css.slice(open + 1, close);
    if (selector.startsWith("@")) {
      if (!OPAQUE_AT_RULE.test(selector)) rules.push(...parseRules(body, options));
    } else if (body.includes("{")) {
      // Dropped under onNesting "skip": the proof layer leaves a rule it
      // cannot represent flat out of the assembled sheet.
      if (options?.onNesting !== "skip") {
        throw new Error(
          `native CSS nesting in the body of "${selector}" is not supported by the walk`,
        );
      }
    } else {
      rules.push({ selector: selector.replace(/\s+/g, " "), body });
    }
    index = close + 1;
  }
  return rules;
}

function blockBodies(css: string, header: RegExp): string[] {
  const bodies: string[] = [];
  for (const match of css.matchAll(header)) {
    const open = css.indexOf("{", match.index + match[0].length - 1);
    if (open < 0) continue;
    bodies.push(css.slice(open + 1, blockEnd(css, open)));
  }
  return bodies;
}

/** Custom properties as one theme defines them: the :root blocks, then the
 * dark block's word when the dark theme is read. One merged map would judge
 * a light-theme size at the dark ramp's value, so the caller picks a theme. */
export function collectTokens(css: string, theme: "light" | "dark" = "light"): Map<string, string> {
  const tokens = new Map<string, string>();
  const add = (body: string): void => {
    for (const m of body.matchAll(/--([a-zA-Z0-9-]+)\s*:\s*([^;]+);/g)) {
      tokens.set(`--${m[1]}`, m[2]!.trim());
    }
  };
  for (const body of blockBodies(css, /:root\s*\{/g)) add(body);
  if (theme === "dark") {
    for (const body of blockBodies(css, /\[data-theme=["']dark["']\]\s*\{/g)) add(body);
  }
  return tokens;
}

function matchingParen(text: string, open: number): number {
  let depth = 0;
  for (let i = open; i < text.length; i += 1) {
    if (text[i] === "(") depth += 1;
    else if (text[i] === ")") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  return -1;
}

function topLevelComma(text: string): number {
  let depth = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "(") depth += 1;
    else if (text[i] === ")") depth -= 1;
    else if (text[i] === "," && depth === 0) return i;
  }
  return -1;
}

/** Split a comma-separated list at the top level: selector lists,
 * math-function arguments, shorthand heads. The paren-depth scan lives here
 * once — typeFloor and cssProof share this instead of carrying their own. */
export function splitTopLevel(text: string): string[] {
  const parts: string[] = [];
  let rest = text;
  for (;;) {
    const comma = topLevelComma(rest);
    if (comma < 0) {
      parts.push(rest.trim());
      break;
    }
    parts.push(rest.slice(0, comma).trim());
    rest = rest.slice(comma + 1);
  }
  return parts.filter((part) => part.length > 0);
}

function resolvePass(
  text: string,
  tokens: ReadonlyMap<string, string>,
  stack: ReadonlySet<string>,
  stuck: Set<string>,
): string {
  let out = "";
  let i = 0;
  while (i < text.length) {
    const at = text.indexOf("var(", i);
    if (at < 0) {
      out += text.slice(i);
      break;
    }
    out += text.slice(i, at);
    const open = at + 3;
    const close = matchingParen(text, open);
    if (close < 0) {
      out += text.slice(at);
      break;
    }
    const inside = text.slice(open + 1, close);
    const comma = topLevelComma(inside);
    const name = (comma < 0 ? inside : inside.slice(0, comma)).trim();
    const fallback = comma < 0 ? null : inside.slice(comma + 1);
    let replacement: string | null = null;
    if (stack.has(name)) {
      stuck.add(name);
    } else if (tokens.has(name)) {
      replacement = resolvePass(tokens.get(name)!, tokens, new Set(stack).add(name), stuck);
    } else if (fallback !== null) {
      replacement = resolvePass(fallback.trim(), tokens, stack, stuck);
    } else {
      stuck.add(name);
    }
    // Each replacement is itself fully resolved, so one left-to-right pass
    // reaches the fixpoint; what stays is exactly the stuck var()s.
    out += replacement ?? text.slice(at, close + 1);
    i = close + 1;
  }
  return out;
}

/** Fold every resolvable var() into its value. `stuck` names the custom
 * properties that stayed behind: unknown, or on a cyclic alias chain. */
export function resolveVars(
  value: string,
  tokens: ReadonlyMap<string, string>,
): { text: string; stuck: Set<string> } {
  const stuck = new Set<string>();
  return { text: resolvePass(value, tokens, new Set(), stuck), stuck };
}
