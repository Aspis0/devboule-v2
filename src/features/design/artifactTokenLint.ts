/*
 * This is a string-level heuristic over untrusted artifact HTML, not a CSS cascade
 * evaluator. It collects definitions anywhere in the text and follows nested var()
 * fallbacks, including references in style="" attributes and content: declarations.
 * Definitions inside @media are treated as available at every width, so conditional
 * resolution remains a known blind spot; malformed markup is tolerated rather than parsed.
 */

const CUSTOM_PROPERTY_PATTERN = /^--[A-Za-z_][A-Za-z0-9_-]*$/;
const CUSTOM_PROPERTY_DEFINITION_PATTERN = /(--[A-Za-z_][A-Za-z0-9_-]*)\s*:/g;
const VAR_FUNCTION_PATTERN = /\bvar\s*\(/gi;

function closingParenthesis(text: string, openIndex: number): number {
  let depth = 0;
  let quote: '"' | "'" | null = null;
  let escaped = false;

  for (let index = openIndex; index < text.length; index += 1) {
    const character = text[index];

    if (quote !== null) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = null;
      }
      continue;
    }

    if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "(") {
      depth += 1;
    } else if (character === ")") {
      depth -= 1;
      if (depth === 0) return index;
    }
  }

  return -1;
}

function fallbackSeparator(text: string): number {
  let depth = 0;
  let quote: '"' | "'" | null = null;
  let escaped = false;

  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];

    if (quote !== null) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = null;
      }
      continue;
    }

    if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "(") {
      depth += 1;
    } else if (character === ")") {
      depth = Math.max(0, depth - 1);
    } else if (character === "," && depth === 0) {
      return index;
    }
  }

  return -1;
}

function definedCustomProperties(text: string): Set<string> {
  const definitions = new Set<string>();
  for (const match of text.matchAll(CUSTOM_PROPERTY_DEFINITION_PATTERN)) {
    definitions.add(match[1]!);
  }
  return definitions;
}

export function findUndefinedCustomProperties(html: string): string[] {
  const defined = definedCustomProperties(html);
  const missing: string[] = [];
  const seen = new Set<string>();

  for (const match of html.matchAll(VAR_FUNCTION_PATTERN)) {
    const functionText = match[0];
    const functionStart = match.index ?? 0;
    const openIndex = functionStart + functionText.lastIndexOf("(");
    const closeIndex = closingParenthesis(html, openIndex);
    if (closeIndex < 0) continue;

    const body = html.slice(openIndex + 1, closeIndex);
    const separator = fallbackSeparator(body);
    const name = body.slice(0, separator < 0 ? body.length : separator).trim();
    if (
      separator < 0 &&
      CUSTOM_PROPERTY_PATTERN.test(name) &&
      !defined.has(name) &&
      !seen.has(name)
    ) {
      seen.add(name);
      missing.push(name);
    }
  }

  return missing;
}
