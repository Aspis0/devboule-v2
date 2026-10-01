import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// Contrast claims for the redesigned palette. SPEC-tokens.md promises ≥4.5:1
// for the text pairs this file walks, in both themes; this suite holds the
// stylesheet to that promise by reading tokens.css — never a copy of it.

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

// ── Parse the light and dark blocks of tokens.css ────────────────────

function parseRootVars(css: string): Map<string, string> {
  const vars = new Map<string, string>();
  const lineRegex = /--([^:\s]+)\s*:\s*([^;]+);/g;
  let match: RegExpExecArray | null;
  while ((match = lineRegex.exec(css)) !== null) {
    vars.set(`--${match[1]!.trim()}`, match[2]!.trim());
  }
  return vars;
}

const CSS_PATH = resolve(import.meta.dirname, "tokens.css");
const css = readFileSync(CSS_PATH, "utf8");

const lightMatch = /:root\s*\{([^}]+)\}/.exec(css);
const darkMatch = /\[data-theme="dark"\]\s*\{([^}]+)\}/.exec(css);

// Each theme's map is the document in cascade order — light: the token root
// then the alias root; dark: both of those plus the dark blocks, which win —
// so a legacy alias resolves through `var()` to its own theme's hex.
const rootBlocks = [...css.matchAll(/:root\s*\{([^}]+)\}/g)].map((m) => m[1]!);
const darkBlocks = [...css.matchAll(/\[data-theme="dark"\]\s*\{([^}]+)\}/g)].map((m) => m[1]!);
const lightVars = parseRootVars(rootBlocks.join("\n"));
const darkVars = parseRootVars([...rootBlocks, ...darkBlocks].join("\n"));

/** Text-bearing pairs SPEC-tokens.md promises at ≥4.5:1, in both themes. */
const CLAIMED_PAIRS: ReadonlyArray<{ text: string; ground: string; why: string }> = [
  { text: "--accent-contrast", ground: "--accent", why: "text on accent fills" },
  {
    text: "--accent",
    ground: "--panel-card",
    why: "the nav-point circle letter — the one text the accent walk allow-lists",
  },
  { text: "--accent-text", ground: "--panel-card", why: "accent text on cards" },
  { text: "--accent-text", ground: "--ground-center", why: "accent text on the transcript" },
  { text: "--accent-text", ground: "--panel-side", why: "accent text on the sidebar" },
  { text: "--accent-text", ground: "--fill-selected", why: "accent text on the selected pill" },
  { text: "--danger-contrast", ground: "--danger", why: "text on filled danger" },
  { text: "--diff-add", ground: "--code-bg", why: "added diff lines on code" },
  { text: "--diff-del", ground: "--code-bg", why: "removed diff lines on code" },
  { text: "--code-text", ground: "--code-bg", why: "code and terminal text" },
  { text: "--ink", ground: "--ground-center", why: "primary text on the transcript" },
  { text: "--ink", ground: "--panel-side", why: "primary text on the panels" },
  { text: "--ink", ground: "--panel-card", why: "primary text on cards" },
  { text: "--ink-soft", ground: "--panel-card", why: "secondary text on cards" },
  { text: "--ink-soft", ground: "--ground-center", why: "secondary text on the transcript" },
  { text: "--muted", ground: "--panel-card", why: "metadata text on cards" },
  {
    text: "--muted",
    ground: "--ground-center",
    why: "metadata and the File tab gutter on the transcript",
  },
  { text: "--tone-attention-text", ground: "--panel-card", why: "attention text on cards" },
  {
    text: "--tone-attention-text",
    ground: "--ground-center",
    why: "attention text on the transcript",
  },
  { text: "--tone-unattended-text", ground: "--panel-card", why: "unattended text on cards" },
  {
    text: "--tone-unattended-text",
    ground: "--ground-center",
    why: "unattended text on the transcript",
  },
];

/**
 * The dark primary ink sits on these grounds as body text; the slice
 * promises ≥7:1 on every one of them, in the dark theme only.
 */
const DARK_INK_FLOOR_7: ReadonlyArray<{ ground: string; why: string }> = [
  { ground: "--ground-app", why: "the app canvas" },
  { ground: "--ground-center", why: "the transcript" },
  { ground: "--panel-side", why: "the sidebar and right panel" },
  { ground: "--panel-card", why: "cards, the composer and user bubbles (one ground)" },
  { ground: "--panel-menu", why: "menus and popovers" },
  { ground: "--fill-selected", why: "the selected fill" },
  { ground: "--fill-selected-soft", why: "multi-selected chips" },
  { ground: "--fill-tool", why: "tool rows" },
  { ground: "--code-bg", why: "code blocks and the terminal ground (one value)" },
];
/**
 * The legacy `*-deep` names stay in service as text tones through the alias
 * block; each must hold ≥4.5:1 on the grounds its consumers paint, in both
 * themes. One alias hop is resolved (`--ochre-deep` →
 * `--tone-attention-text` → hex) — the stylesheet's own chain, never a copy.
 */
const LEGACY_TEXT_TONES = ["--green-deep", "--purple-deep", "--ochre-deep", "--danger-deep"];
const LEGACY_GROUNDS = ["--panel-card", "--ground-center"];

function resolveToken(name: string, vars: Map<string, string>): string {
  const value = vars.get(name);
  if (value === undefined) throw new Error(`${name} is not defined`);
  const ref = /^var\((--[a-z-]+)\)$/.exec(value);
  return ref === null ? value : resolveToken(ref[1]!, vars);
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

describe("palette contrast (both themes, from tokens.css)", () => {
  it("parsed both theme blocks with their colour tokens present", () => {
    expect(lightMatch, ":root block not found").not.toBeNull();
    expect(darkMatch, "[data-theme=dark] block not found").not.toBeNull();
    expect(lightVars.size).toBeGreaterThan(40);
    expect(darkVars.size).toBeGreaterThan(40);
  });

  for (const [theme, vars] of [
    ["light", lightVars],
    ["dark", darkVars],
  ] as const) {
    for (const pair of CLAIMED_PAIRS) {
      it(`${theme}: ${pair.text} on ${pair.ground} ≥ 4.5 (${pair.why})`, () => {
        const text = vars.get(pair.text);
        const ground = vars.get(pair.ground);
        expect(text, `${pair.text} missing from the ${theme} block`).toMatch(/^#[0-9a-fA-F]{6}$/);
        expect(ground, `${pair.ground} missing from the ${theme} block`).toMatch(
          /^#[0-9a-fA-F]{6}$/,
        );
        const ratio = contrastRatio(text!, ground!);
        expect(ratio, `${pair.text} ${text} on ${pair.ground} ${ground}`).toBeGreaterThanOrEqual(
          4.5,
        );
      });
    }

    for (const tone of LEGACY_TEXT_TONES) {
      for (const ground of LEGACY_GROUNDS) {
        it(`${theme}: legacy ${tone} resolves to text ≥ 4.5 on ${ground}`, () => {
          const text = resolveToken(tone, vars);
          const groundHex = resolveToken(ground, vars);
          expect(text, `${tone} does not resolve to a hex colour`).toMatch(/^#[0-9a-fA-F]{6}$/);
          const ratio = contrastRatio(text, groundHex);
          expect(ratio, `${tone} (${text}) on ${ground} (${groundHex})`).toBeGreaterThanOrEqual(
            4.5,
          );
        });
      }
    }
  }

  for (const entry of DARK_INK_FLOOR_7) {
    it(`dark: --ink on ${entry.ground} ≥ 7 (${entry.why})`, () => {
      const text = darkVars.get("--ink");
      const ground = darkVars.get(entry.ground);
      expect(text, "--ink missing from the dark block").toMatch(/^#[0-9a-fA-F]{6}$/);
      expect(ground, `${entry.ground} missing from the dark block`).toMatch(/^#[0-9a-fA-F]{6}$/);
      const ratio = contrastRatio(text!, ground!);
      expect(ratio, `--ink ${text} on ${entry.ground} ${ground}`).toBeGreaterThanOrEqual(7);
    });
  }

  it("dark: --ink on the inline-code fill over the transcript ≥ 7 (derived ground)", () => {
    const text = darkVars.get("--ink");
    const centre = darkVars.get("--ground-center");
    const fill = darkVars.get("--fill-code-inline");
    expect(text, "--ink missing from the dark block").toMatch(/^#[0-9a-fA-F]{6}$/);
    expect(centre, "--ground-center missing from the dark block").toMatch(/^#[0-9a-fA-F]{6}$/);
    // The fill paints with `background:`, replacing the element's own paint:
    // it composites over the parent's transcript ground, never over itself.
    const percent = Number(/var\(--ink\)\s*([\d.]+)%/.exec(fill!)?.[1]);
    expect(percent, `--fill-code-inline is not an ink mix: ${fill}`).toBeGreaterThan(0);
    const ground = mixOver(text!, centre!, percent);
    const ratio = contrastRatio(text!, ground);
    expect(ratio, `--ink ${text} on inline-code fill ${ground}`).toBeGreaterThanOrEqual(7);
  });
});
