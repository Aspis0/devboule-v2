import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { contrastRatio, HEX_COLOR } from "./contrast";

// Contrast claims for the redesigned palette. SPEC-tokens.md promises ≥4.5:1
// for the text pairs this file walks, in both themes; this suite holds the
// stylesheet to that promise by reading tokens.css — never a copy of it.

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
  { text: "--accent-text", ground: "--panel-card", why: "accent text on cards" },
  { text: "--accent-text", ground: "--ground-center", why: "accent text on the transcript" },
  { text: "--accent-text", ground: "--panel-side", why: "accent text on the sidebar" },
  { text: "--accent-text", ground: "--fill-selected", why: "accent text on the selected pill" },
  { text: "--danger-contrast", ground: "--danger", why: "text on filled danger" },
  { text: "--danger", ground: "--panel-menu", why: "failure text in menus" },
  { text: "--danger", ground: "--panel-card", why: "failure text on cards and the surface" },
  {
    text: "--accent-on-code",
    ground: "--code-bg",
    why: "the terminal's magenta and cursor on code",
  },
  { text: "--diff-add", ground: "--code-bg", why: "added diff lines on code" },
  { text: "--diff-del", ground: "--code-bg", why: "removed diff lines on code" },
  { text: "--code-text", ground: "--code-bg", why: "code and terminal text" },
  { text: "--ink", ground: "--ground-center", why: "primary text on the transcript" },
  { text: "--ink", ground: "--panel-side", why: "primary text on the panels" },
  { text: "--ink", ground: "--panel-card", why: "primary text on cards" },
  { text: "--ink-soft", ground: "--panel-card", why: "secondary text on cards" },
  { text: "--ink-soft", ground: "--ground-center", why: "secondary text on the transcript" },
  { text: "--muted", ground: "--panel-card", why: "metadata text on cards" },
  { text: "--muted", ground: "--panel-menu", why: "secondary text in menus and popovers" },
  { text: "--muted", ground: "--fill-tool", why: "secondary text on tool rows" },
  { text: "--muted", ground: "--fill-selected", why: "secondary text on the selected row" },
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
 * Secondary text at the reference 6.97 floor, per theme and ground: the
 * grounds dark muted clears it on (app, transcript, side, selected row) and
 * the ones light muted clears it on (app, transcript, side, cards, tool rows,
 * selected row). Dark cards, menus, tool rows and the multi-select fill stay
 * below the reference floor — they hold the 4.5 SPEC pairs below, never less.
 */
const SECONDARY_FLOOR_697: ReadonlyArray<{
  theme: "light" | "dark";
  ground: string;
  why: string;
}> = [
  { theme: "dark", ground: "--ground-app", why: "secondary text on the app ground" },
  { theme: "dark", ground: "--ground-center", why: "secondary text on the transcript" },
  { theme: "dark", ground: "--panel-side", why: "secondary text on the sidebar" },
  { theme: "light", ground: "--ground-app", why: "secondary text on the app ground" },
  { theme: "light", ground: "--ground-center", why: "secondary text on the transcript" },
  { theme: "light", ground: "--panel-side", why: "secondary text on the sidebar" },
  { theme: "light", ground: "--panel-card", why: "secondary text on cards" },
  { theme: "light", ground: "--fill-tool", why: "secondary text on tool rows" },
  { theme: "light", ground: "--fill-selected", why: "secondary text on the selected row" },
  { theme: "dark", ground: "--fill-selected", why: "secondary text on the selected row" },
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
  { ground: "--code-bg", why: "code blocks" },
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

/**
 * The selected row's fill as a colour: a named hex, or the one mix the light
 * theme writes for it — the pair below is judged on what is painted, not on
 * how the declaration spells it.
 */
function selectedFill(fill: string, vars: Map<string, string>): string {
  if (/^#[0-9a-fA-F]{6}$/.test(fill)) return fill;
  const mix = /^color-mix\(in srgb,\s*(#[0-9a-fA-F]{6})\s+(\d+)%,\s*var\((--[a-z-]+)\)\)$/.exec(
    fill,
  );
  expect(
    mix,
    `--fill-selected-soft is neither a hex nor the mix this resolves: ${fill}`,
  ).not.toBeNull();
  const base = vars.get(mix![3]!);
  expect(base, `${mix![3]} missing from the block`).toMatch(/^#[0-9a-fA-F]{6}$/);
  return mixOver(mix![1]!, base!, Number(mix![2]!));
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

  for (const pair of SECONDARY_FLOOR_697) {
    const vars = pair.theme === "light" ? lightVars : darkVars;
    it(`${pair.theme}: --muted on ${pair.ground} ≥ 6.97 (${pair.why})`, () => {
      const text = vars.get("--muted");
      const ground = vars.get(pair.ground);
      expect(text, `--muted missing from the ${pair.theme} block`).toMatch(/^#[0-9a-fA-F]{6}$/);
      expect(ground, `${pair.ground} missing from the ${pair.theme} block`).toMatch(
        /^#[0-9a-fA-F]{6}$/,
      );
      const ratio = contrastRatio(text!, ground!);
      expect(ratio, `--muted ${text} on ${pair.ground} ${ground}`).toBeGreaterThanOrEqual(6.97);
    });
  }

  // The selected row paints its fact with the primary ink — no muted tone
  // clears the floor on that fill, so the fact brightens with the row — and
  // the pair is judged on both themes' own fill.
  for (const [theme, vars] of [
    ["light", lightVars],
    ["dark", darkVars],
  ] as const) {
    it(`${theme}: --ink on the selected row's fill ≥ 6.97`, () => {
      const text = vars.get("--ink");
      expect(text, `--ink missing from the ${theme} block`).toMatch(/^#[0-9a-fA-F]{6}$/);
      const ground = selectedFill(vars.get("--fill-selected-soft") ?? "", vars);
      const ratio = contrastRatio(text!, ground);
      expect(ratio, `--ink ${text} on --fill-selected-soft ${ground}`).toBeGreaterThanOrEqual(6.97);
    });
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

/**
 * The selected row's own fill, and the reference 6.97 floor for the ink on it:
 * the row is the one ground whose fill changes when the strip is on it, and a
 * person reads the tab name there more than anywhere else in the bar.
 */
const SELECTED_ROW_FLOORS: ReadonlyArray<{ theme: "light" | "dark"; text: string; why: string }> = [
  { theme: "light", text: "--ink", why: "the tab name on the selected row" },
  { theme: "light", text: "--ink-soft", why: "secondary text on the selected row" },
  { theme: "dark", text: "--ink", why: "the tab name on the selected row" },
  { theme: "dark", text: "--ink-soft", why: "secondary text on the selected row" },
];

/**
 * The focus ring (`--ring`, the accent by default) and the ground it is drawn
 * on. The surfaces that stay dark in both themes set `--ring` to
 * `--accent-on-code`; ringConsumers.walk.test.ts proves the stylesheet does
 * that wherever such a ground is painted, so this table only holds the colour
 * pairs themselves.
 */
const RING_ON_GROUNDS: ReadonlyArray<{ ring: string; ground: string; why: string }> = [
  { ring: "--ring", ground: "--ground-app", why: "the app canvas" },
  { ring: "--ring", ground: "--ground-center", why: "the transcript" },
  { ring: "--ring", ground: "--panel-side", why: "the sidebar and right panel" },
  { ring: "--ring", ground: "--panel-card", why: "cards, the composer and bubbles" },
  { ring: "--ring", ground: "--panel-menu", why: "menus and popovers" },
  { ring: "--ring", ground: "--fill-selected", why: "the selected row" },
  { ring: "--ring", ground: "--fill-selected-soft", why: "multi-selected chips" },
  { ring: "--ring", ground: "--fill-tool", why: "tool rows and the copyable block" },
  { ring: "--accent-on-code", ground: "--code-bg", why: "code blocks" },
  { ring: "--ring", ground: "--terminal-ground", why: "the terminal" },
];

describe("the selected row and the focus ring", () => {
  for (const entry of SELECTED_ROW_FLOORS) {
    const vars = entry.theme === "light" ? lightVars : darkVars;
    it(`${entry.theme}: ${entry.text} on --fill-selected ≥ 6.97 (${entry.why})`, () => {
      const text = vars.get(entry.text);
      const ground = vars.get("--fill-selected");
      expect(text, `${entry.text} missing from the ${entry.theme} block`).toMatch(
        /^#[0-9a-fA-F]{6}$/,
      );
      expect(ground, `--fill-selected missing from the ${entry.theme} block`).toMatch(
        /^#[0-9a-fA-F]{6}$/,
      );
      const ratio = contrastRatio(text!, ground!);
      expect(ratio, `${entry.text} ${text} on --fill-selected ${ground}`).toBeGreaterThanOrEqual(
        6.97,
      );
    });
  }

  for (const entry of RING_ON_GROUNDS) {
    for (const [theme, vars] of [
      ["light", lightVars],
      ["dark", darkVars],
    ] as const) {
      it(`${theme}: ${entry.ring} ≥ 3 on ${entry.ground} (${entry.why})`, () => {
        const ring = resolveToken(entry.ring, vars);
        const ground = resolveToken(entry.ground, vars);
        expect(ring, `${entry.ring} does not resolve to a hex colour`).toMatch(HEX_COLOR);
        expect(ground, `${entry.ground} does not resolve to a hex colour`).toMatch(HEX_COLOR);
        expect(
          contrastRatio(ring, ground),
          `${entry.ring} ${ring} on ${entry.ground} ${ground}`,
        ).toBeGreaterThanOrEqual(3);
      });
    }
  }

  it("the default ring is the accent", () => {
    for (const vars of [lightVars, darkVars]) {
      expect(resolveToken("--ring", vars)).toBe(resolveToken("--accent", vars));
    }
  });
});

/**
 * What a terminal paints on its own ground in each theme: the default
 * foreground, the dim text and the ANSI tones a shell prints for prompts,
 * errors and warnings. Each one reads as text on `--terminal-ground`.
 */
const TERMINAL_TEXT_ON_GROUND: ReadonlyArray<{ text: string; why: string }> = [
  { text: "--ink", why: "the default foreground and bright white" },
  { text: "--terminal-dim", why: "ANSI bright black" },
  { text: "--danger", why: "ANSI red" },
  { text: "--tone-live", why: "ANSI green and cyan" },
  { text: "--tone-attention-text", why: "ANSI yellow" },
  { text: "--tone-unattended-text", why: "ANSI blue" },
  { text: "--ring", why: "the cursor and ANSI magenta" },
];

describe("the terminal's text on its ground (both themes, from tokens.css)", () => {
  for (const [theme, vars] of [
    ["light", lightVars],
    ["dark", darkVars],
  ] as const) {
    for (const entry of TERMINAL_TEXT_ON_GROUND) {
      it(`${theme}: ${entry.text} on --terminal-ground ≥ 4.5 (${entry.why})`, () => {
        const text = resolveToken(entry.text, vars);
        const ground = resolveToken("--terminal-ground", vars);
        expect(text, `${entry.text} does not resolve to a hex colour`).toMatch(HEX_COLOR);
        expect(ground, "--terminal-ground does not resolve to a hex colour").toMatch(HEX_COLOR);
        expect(
          contrastRatio(text, ground),
          `${entry.text} ${text} on --terminal-ground ${ground}`,
        ).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
});

/**
 * A status dot is 6 px and carries its state by colour alone, so it answers to
 * the non-text floor (3:1), not the text one. The failed dot is judged on every
 * ground it is painted on: the strip, the pane header, the subagent menu.
 */
const FAILED_DOT_GROUNDS: ReadonlyArray<{ ground: string; why: string }> = [
  { ground: "--ground-app", why: "the app canvas" },
  { ground: "--ground-center", why: "the transcript" },
  { ground: "--panel-side", why: "the sidebar and the strip" },
  { ground: "--panel-card", why: "cards" },
  { ground: "--panel-menu", why: "the subagent menu" },
  { ground: "--fill-selected", why: "the selected row" },
];

describe("status dot contrast (both themes, from tokens.css)", () => {
  for (const [theme, vars] of [
    ["light", lightVars],
    ["dark", darkVars],
  ] as const) {
    for (const entry of FAILED_DOT_GROUNDS) {
      it(`${theme}: --tone-failed ≥ 3 on ${entry.ground} (${entry.why})`, () => {
        const dot = resolveToken("--tone-failed", vars);
        const ground = resolveToken(entry.ground, vars);
        expect(dot, "--tone-failed does not resolve to a hex colour").toMatch(HEX_COLOR);
        expect(ground, `${entry.ground} does not resolve to a hex colour`).toMatch(HEX_COLOR);
        expect(
          contrastRatio(dot, ground),
          `--tone-failed ${dot} on ${entry.ground} ${ground}`,
        ).toBeGreaterThanOrEqual(3);
      });
    }
  }
});

describe("palette A neutrality (both themes, from tokens.css)", () => {
  /**
   * Every structural surface stays near-neutral: ochre lives only in the
   * accent and tone tokens, never in a ground, panel, line or fill.
   * Chroma is the sRGB channel spread; the old warm grounds spread 21+
   * (e.g. --line #ded6c4 at 26) while the new ones stay at 12 or under.
   */
  const NEAR_NEUTRAL_SURFACES = [
    "--ground-app",
    "--ground-center",
    "--panel-side",
    "--panel-card",
    "--panel-composer",
    "--panel-menu",
    "--line",
    "--line-strong",
    "--fill-selected",
    "--fill-selected-soft",
    "--fill-tool",
    "--fill-plus",
    "--bubble-bg",
    "--code-bg",
    "--terminal-ground",
  ] as const;
  const CHROMA_CAP = 16;

  function chroma(hex: string): number {
    const channels = [1, 3, 5].map((at) => parseInt(hex.slice(at, at + 2), 16));
    return Math.max(...channels) - Math.min(...channels);
  }

  for (const [theme, vars] of [
    ["light", lightVars],
    ["dark", darkVars],
  ] as const) {
    for (const name of NEAR_NEUTRAL_SURFACES) {
      it(`${theme}: ${name} is near-neutral (chroma ≤ ${CHROMA_CAP})`, () => {
        const value = vars.get(name);
        expect(value, `${name} missing from the ${theme} block`).toMatch(/^#[0-9a-fA-F]{6}$/);
        expect(chroma(value!), `${name} (${value}) exceeds the neutral cap`).toBeLessThanOrEqual(
          CHROMA_CAP,
        );
      });
    }
  }
});
