import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// The Diff tab paints tone text on tone-tinted grounds (SPEC-regions §Right
// panel: 22% add, 18% del, ochre hunk). Both themes mix the texts from tone
// and ink with `color-mix`, and the line numbers read `--tone-idle-text`;
// this suite resolves those expressions straight from the stylesheets — never
// a copy — holds the pairings to 4.5:1 in both themes, and pins the ratios.

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
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

/** `color-mix(in srgb, …)` mixes encoded channels — plain channel lerp. */
function mixOver(fg: string, bg: string, percent: number): string {
  const p = percent / 100;
  const channels = [1, 3, 5].map((at) => {
    const f = parseInt(fg.slice(at, at + 2), 16);
    const b = parseInt(bg.slice(at, at + 2), 16);
    return Math.round(f * p + b * (1 - p))
      .toString(16)
      .padStart(2, "0");
  });
  return `#${channels.join("")}`;
}

// ── Read both stylesheets ────────────────────────────────────────────

const TOKENS_PATH = resolve(import.meta.dirname, "../../styles/tokens.css");
const TAB_CSS_PATH = resolve(import.meta.dirname, "./panel/diffTab.css");
const tokensCss = readFileSync(TOKENS_PATH, "utf8");
const tabCss = readFileSync(TAB_CSS_PATH, "utf8");

function themeVars(theme: "light" | "dark"): Map<string, string> {
  const rootBlocks = [...tokensCss.matchAll(/:root\s*\{([^}]+)\}/g)].map((m) => m[1]!);
  const darkBlocks = [...tokensCss.matchAll(/\[data-theme="dark"\]\s*\{([^}]+)\}/g)].map(
    (m) => m[1]!,
  );
  const blocks =
    theme === "light" ? rootBlocks.join("\n") : [...rootBlocks, ...darkBlocks].join("\n");
  const vars = new Map<string, string>();
  const lineRegex = /--([^:\s]+)\s*:\s*([^;]+);/g;
  let match: RegExpExecArray | null;
  while ((match = lineRegex.exec(blocks)) !== null) {
    vars.set(`--${match[1]!.trim()}`, match[2]!.trim());
  }
  return vars;
}

/** Resolve a token to hex: a literal, one `var()` hop, or a `color-mix`. */
function resolveHex(name: string, vars: Map<string, string>): string {
  const value = vars.get(name);
  if (value === undefined) throw new Error(`${name} is not defined`);
  const ref = /^var\((--[a-z-]+)\)$/.exec(value);
  if (ref !== null) return resolveHex(ref[1]!, vars);
  const mix = /^color-mix\(in srgb,\s*var\((--[a-z-]+)\)\s*([\d.]+)%,\s*var\((--[a-z-]+)\)\)$/.exec(
    value,
  );
  if (mix !== null) {
    return mixOver(resolveHex(mix[1]!, vars), resolveHex(mix[3]!, vars), Number(mix[2]));
  }
  return value;
}

function ruleBody(selector: string): string {
  const match = tabCss.match(new RegExp(`\\${selector}\\s*\\{([^}]*)\\}`));
  if (match === null) throw new Error(`${selector} rule not found in diffTab.css`);
  return match[1]!;
}

/** The tint token's mix: which tone over which ground at which percent. */
function tintMix(
  token: string,
  vars: Map<string, string>,
): { fg: string; bg: string; pct: number } {
  const value = vars.get(token);
  if (value === undefined) throw new Error(`${token} is not defined`);
  const mix = /color-mix\(in srgb,\s*var\((--[a-z-]+)\)\s*([\d.]+)%,\s*var\((--[a-z-]+)\)\)/.exec(
    value,
  );
  if (mix === null) throw new Error(`${token} is not a tone-over-ground mix: ${value}`);
  const fg = resolveHex(mix[1]!, vars);
  const bg = resolveHex(mix[3]!, vars);
  return { fg, bg, pct: Number(mix[2]) };
}

const PAIRS = [
  {
    row: ".diff-tab-added",
    groundToken: "--diff-row-add",
    textToken: "--diff-text-add",
    edgeToken: "--tone-add",
    why: "added rows",
  },
  {
    row: ".diff-tab-removed",
    groundToken: "--diff-row-del",
    textToken: "--diff-text-del",
    edgeToken: "--tone-del",
    why: "removed rows",
  },
  {
    row: ".diff-tab-hunk",
    groundToken: "--diff-row-hunk",
    textToken: "--diff-text-hunk",
    edgeToken: null,
    why: "hunk headers",
  },
] as const;

const EXPECTED_RATIOS: Record<string, Record<string, string>> = {
  light: {
    "--diff-text-add": "6.01",
    "--diff-text-del": "6.07",
    "--diff-text-hunk": "5.29",
  },
  dark: {
    "--diff-text-add": "6.49",
    "--diff-text-del": "6.65",
    "--diff-text-hunk": "8.84",
  },
};

// The numbers paint on the bare body and on every row tint: the hunk row
// carries no numbers today, but its tint is still a ground they could land on.
const EXPECTED_NUMBER_RATIOS: Record<string, Record<string, string>> = {
  light: {
    "--ground-center": "6.30",
    "--diff-row-add": "4.60",
    "--diff-row-del": "4.84",
    "--diff-row-hunk": "5.81",
  },
  dark: {
    "--ground-center": "7.24",
    "--diff-row-add": "4.73",
    "--diff-row-del": "5.43",
    "--diff-row-hunk": "6.11",
  },
};

describe("diff tab tint contrast (both themes, from the stylesheets)", () => {
  for (const pair of PAIRS) {
    it(`${pair.row} paints the measured ${pair.why} tokens`, () => {
      const body = ruleBody(pair.row);
      expect(body).toContain(`background: var(${pair.groundToken})`);
      expect(body).toContain(`color: var(${pair.textToken})`);
      if (pair.edgeToken !== null) {
        expect(body).toContain(`box-shadow: inset 2px 0 0 var(${pair.edgeToken})`);
      }
    });
  }

  it("paints line numbers from --tone-idle-text with no own background", () => {
    const body = ruleBody(".diff-tab-num");
    expect(body).toContain("color: var(--tone-idle-text)");
    // The numbers sit on the row's tint: their own background would void
    // every ground the suite below measures them on.
    expect(body).not.toContain("background");
  });

  it("mixes each tint from its tone over the centre ground at the spec percent", () => {
    const expected: Record<string, { tone: string; pct: number }> = {
      "--diff-row-add": { tone: "--tone-add", pct: 22 },
      "--diff-row-del": { tone: "--tone-del", pct: 18 },
      "--diff-row-hunk": { tone: "--tone-warn", pct: 10 },
    };
    for (const theme of ["light", "dark"] as const) {
      const vars = themeVars(theme);
      for (const [token, want] of Object.entries(expected)) {
        const value = vars.get(token);
        expect(value).toContain(`var(${want.tone}) ${want.pct}%`);
        expect(value).toContain("var(--ground-center)");
      }
    }
  });

  for (const theme of ["light", "dark"] as const) {
    for (const pair of PAIRS) {
      it(`${theme}: ${pair.textToken} on ${pair.groundToken} ≥ 4.5 (${pair.why})`, () => {
        const vars = themeVars(theme);
        const text = resolveHex(pair.textToken, vars);
        const { fg, bg, pct } = tintMix(pair.groundToken, vars);
        expect(text).toMatch(/^#[0-9a-fA-F]{6}$/);
        const ground = mixOver(fg, bg, pct);
        const ratio = contrastRatio(text, ground);
        expect(
          ratio,
          `${pair.textToken} ${text} on ${pair.groundToken} ${ground} (${theme})`,
        ).toBeGreaterThanOrEqual(4.5);
        expect(ratio.toFixed(2)).toBe(EXPECTED_RATIOS[theme][pair.textToken]);
      });
    }

    it(`${theme}: --tone-idle-text on every number ground ≥ 4.5 (line numbers)`, () => {
      const vars = themeVars(theme);
      const text = resolveHex("--tone-idle-text", vars);
      expect(text).toMatch(/^#[0-9a-fA-F]{6}$/);
      // The grounds as rendered: the bare body plus each tint composited
      // over it, resolved from the same tokens the stylesheet uses.
      const grounds = new Map<string, string>([
        ["--ground-center", resolveHex("--ground-center", vars)],
      ]);
      for (const pair of PAIRS) {
        const { fg, bg, pct } = tintMix(pair.groundToken, vars);
        grounds.set(pair.groundToken, mixOver(fg, bg, pct));
      }
      expect(grounds.size).toBe(4);
      for (const [name, ground] of grounds) {
        const ratio = contrastRatio(text, ground);
        expect(
          ratio,
          `--tone-idle-text ${text} on ${name} ${ground} (${theme})`,
        ).toBeGreaterThanOrEqual(4.5);
        expect(ratio.toFixed(2)).toBe(EXPECTED_NUMBER_RATIOS[theme]![name]);
      }
    });
  }
});
