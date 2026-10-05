// The goal row's measured contract, straight from the stylesheets — never
// a copy: the label and the objective hold 4.5:1 on the centre ground in
// both themes, the type tokens resolve at or above the 12px floor, and the
// row keeps the spec's 32px minimum and bottom border. The target icon is
// decorative (aria-hidden, like the other SVG marks), so its ratio is pinned
// but not gated.
// @vitest-environment node
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
  return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
}

// ── Read both stylesheets ────────────────────────────────────────────

const TOKENS_PATH = resolve(import.meta.dirname, "../../../styles/tokens.css");
const GOAL_CSS_PATH = resolve(import.meta.dirname, "./GoalLine.css");
const tokensCss = readFileSync(TOKENS_PATH, "utf8");
const goalCss = readFileSync(GOAL_CSS_PATH, "utf8");

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

function ruleBody(selector: string): string {
  const match = goalCss.match(new RegExp(`\\${selector}\\s*\\{([^}]*)\\}`));
  if (match === null) throw new Error(`${selector} rule not found in GoalLine.css`);
  return match[1]!;
}

const EXPECTED_RATIOS: Record<string, Record<string, string>> = {
  light: {
    "--muted": "7.04",
    "--ink-soft": "7.15",
    "--tone-live": "3.87",
  },
  dark: {
    "--muted": "7.41",
    "--ink-soft": "9.67",
    "--tone-live": "7.79",
  },
};

describe("the goal row's painted contract", () => {
  it("paints the label muted at the meta size and the objective ink-soft at small", () => {
    const label = ruleBody(".goal-line-label");
    expect(label).toContain("color: var(--muted)");
    expect(label).toContain("font-size: var(--type-meta)");
    const text = ruleBody(".goal-line-text");
    expect(text).toContain("color: var(--ink-soft)");
    expect(text).toContain("font-size: var(--type-small)");
  });

  it("keeps the collapsed objective to one ellipsised line", () => {
    const text = ruleBody(".goal-line-text");
    expect(text).toContain("white-space: nowrap");
    expect(text).toContain("text-overflow: ellipsis");
    expect(text).toContain("overflow: hidden");
  });

  it("wraps the whole objective once expanded", () => {
    const expanded = ruleBody(".goal-line.is-expanded .goal-line-text");
    expect(expanded).toContain("white-space: normal");
  });

  it("keeps the spec's 32px minimum and the bottom border", () => {
    const row = ruleBody(".goal-line");
    expect(row).toContain("min-height: var(--control-large);");
    expect(row).toContain("border-bottom: 1px solid var(--line)");
  });

  it("paints the target icon in the live tone at 14px", () => {
    const icon = ruleBody(".goal-line-icon");
    expect(icon).toContain("color: var(--tone-live)");
    expect(icon).toContain("width: 14px");
    expect(icon).toContain("height: 14px");
  });

  it("resolves both type tokens at or above the 12px floor", () => {
    for (const theme of ["light", "dark"] as const) {
      const vars = themeVars(theme);
      for (const token of ["--type-meta", "--type-small"]) {
        const value = vars.get(token);
        expect(value).toMatch(/^\d+(\.\d+)?px$/);
        expect(Number.parseFloat(value!)).toBeGreaterThanOrEqual(12);
      }
    }
    expect(themeVars("light").get("--type-meta")).toBe("12px");
    expect(themeVars("light").get("--type-small")).toBe("13px");
  });

  for (const theme of ["light", "dark"] as const) {
    for (const token of ["--muted", "--ink-soft"] as const) {
      it(`${theme}: ${token} on the centre ground ≥ 4.5`, () => {
        const vars = themeVars(theme);
        const text = vars.get(token);
        const ground = vars.get("--ground-center");
        expect(text).toMatch(/^#[0-9a-fA-F]{6}$/);
        expect(ground).toMatch(/^#[0-9a-fA-F]{6}$/);
        const ratio = contrastRatio(text!, ground!);
        expect(
          ratio,
          `${token} ${text} on --ground-center ${ground} (${theme})`,
        ).toBeGreaterThanOrEqual(4.5);
        expect(ratio.toFixed(2)).toBe(EXPECTED_RATIOS[theme]![token]);
      });
    }

    it(`${theme}: pins the decorative icon ratio (no gate)`, () => {
      const vars = themeVars(theme);
      const ratio = contrastRatio(vars.get("--tone-live")!, vars.get("--ground-center")!);
      expect(ratio.toFixed(2)).toBe(EXPECTED_RATIOS[theme]!["--tone-live"]);
    });
  }
});
