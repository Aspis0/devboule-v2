import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// ── sRGB → linear → WCAG 2.1 relative luminance ──────────────────────

function linearize(channel: number): number {
  const c = channel / 255;
  return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function relativeLuminance(hex: string): number {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return 0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b);
}

function contrastRatio(hexA: string, hexB: string): number {
  const l1 = relativeLuminance(hexA);
  const l2 = relativeLuminance(hexB);
  const lighter = Math.max(l1, l2);
  const darker = Math.min(l1, l2);
  return (lighter + 0.05) / (darker + 0.05);
}

// ── Parse :root custom properties from global.css ────────────────────

interface CssVar {
  name: string;
  value: string;
  comment: string | null;
}

function parseRootVars(css: string): CssVar[] {
  const vars: CssVar[] = [];
  const rootMatch = css.match(/:root\s*\{([^}]+)\}/);
  if (!rootMatch) return vars;

  const block = rootMatch[1]!;
  // Match each custom property: --name: value; /* optional comment */
  const lineRegex = /--([^:]+):\s*([^;]+);\s*(?:\/\*\s*(.*?)\s*\*\/)?/g;
  let match: RegExpExecArray | null;
  while ((match = lineRegex.exec(block)) !== null) {
    vars.push({
      name: `--${match[1]!.trim()}`,
      value: match[2]!.trim(),
      comment: match[3]?.trim() ?? null,
    });
  }
  return vars;
}

/** Extract the claimed ratio from a `/* worst-case N.NN *&#47; comment. */
function parseClaimed(comment: string | null): number | null {
  if (comment === null) return null;
  const match = comment.match(/^worst-case\s+(\d+\.\d+)$/);
  return match !== null ? Number.parseFloat(match[1]!) : null;
}

// Palette surfaces where text is painted. --selection is the darkest, so for
// dark text it always yields the minimum ratio — the "worst case".
//
// Which tokens count as a text background is a role judgement the stylesheet
// does not encode, so the names are listed here. Their colours deliberately are
// not: a hardcoded background would keep passing after somebody edited the
// palette, which is exactly the rot this file exists to catch.
const TEXT_SURFACE_NAMES = [
  "--white",
  "--surface",
  "--surface-muted",
  "--sand",
  "--selection",
] as const;

// ── Tests ────────────────────────────────────────────────────────────

const CSS_PATH = resolve(import.meta.dirname, "global.css");

describe("palette contrast", () => {
  const css = readFileSync(CSS_PATH, "utf8");
  const vars = parseRootVars(css);
  const varMap = new Map<string, CssVar>();
  for (const v of vars) varMap.set(v.name, v);

  it("parsed the :root block successfully", () => {
    // Parsing must find a plausible number of tokens (not zero) and
    // must find every token this suite depends on.
    expect(vars.length).toBeGreaterThan(20);
    for (const name of [
      "--green-deep",
      "--purple-deep",
      "--ochre-deep",
      "--danger-deep",
      "--terracotta-deep",
    ]) {
      expect(varMap.has(name), `${name} not found in :root`).toBe(true);
    }
    for (const name of TEXT_SURFACE_NAMES) {
      expect(varMap.has(name), `${name} not found in :root`).toBe(true);
    }
  });

  /** The surface's colour as the stylesheet currently defines it, never a copy. */
  function surfaceHex(name: string): string {
    const value = varMap.get(name)?.value;
    if (value === undefined || !/^#[0-9a-fA-F]{6}$/.test(value)) {
      throw new Error(`${name} is not a six-digit hex colour in :root`);
    }
    return value;
  }

  // ── Every *-deep token carries a verified worst-case claim ──────

  const deepTokens = vars.filter(
    (v) => v.name.endsWith("-deep") && /^#[0-9a-fA-F]{6}$/.test(v.value),
  );
  const missingClaim = deepTokens.filter((v) => parseClaimed(v.comment) === null);

  for (const token of deepTokens) {
    const claimed = parseClaimed(token.comment);
    if (claimed === null) continue; // handled by the structural check below

    it(`${token.name} (${token.value}) worst-case ratio matches claim ${claimed}`, () => {
      // Compute ratio against every text-bearing surface; the minimum
      // is the worst case (darkest background gives smallest ratio for
      // dark text).
      const ratios = TEXT_SURFACE_NAMES.map((name) => contrastRatio(token.value, surfaceHex(name)));
      const worst = Math.min(...ratios);
      const rounded = Math.round(worst * 100) / 100;
      expect(rounded).toBe(claimed);
    });
  }

  it("every *-deep token carries a /* worst-case N.NN */ comment", () => {
    if (missingClaim.length > 0) {
      const names = missingClaim.map((v) => v.name).join(", ");
      expect.fail(
        `${missingClaim.length} *-deep token(s) without a worst-case comment: ${names}. ` +
          "Every contrast-safe variant should carry a verified claim.",
      );
    }
  });
});
