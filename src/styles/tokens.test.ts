import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { GROUND_BY_THEME } from "../lib/theme";

const CSS_PATH = resolve(import.meta.dirname, "tokens.css");
const GLOBAL_CSS_PATH = resolve(import.meta.dirname, "global.css");

/** Every colour token of SPEC-tokens.md whose value differs between the themes. */
const COLOUR_TOKENS = [
  "ground-app",
  "ground-center",
  "panel-side",
  "panel-card",
  "panel-composer",
  "panel-menu",
  "fill-hover",
  "fill-chip-hover",
  "fill-selected",
  "fill-selected-soft",
  "fill-plus",
  "fill-tool",
  "bubble-bg",
  "code-bg",
  "code-text",
  "fill-code-inline",
  "diff-add",
  "diff-del",
  "ink",
  "ink-soft",
  "muted",
  "line",
  "line-strong",
  "region-edge",
  "accent",
  "accent-contrast",
  "accent-soft",
  "tone-live",
  "tone-idle",
  "tone-attention",
  "tone-unattended",
  "tone-recovered",
  "danger",
  "danger-contrast",
  "tone-add",
  "tone-del",
  "tone-warn",
  "alert-border",
  "alert-bg",
  "scrim",
  "lb-text",
  "lb-hover",
  "shadow-pop",
  "terminal-ground",
  "tone-attention-text",
  "tone-unattended-text",
] as const;

/** Theme-independent tokens: fonts, ramp, spacing, radii, heights, motion, frame hooks. */
const SHARED_TOKENS = [
  "font-ui",
  "font-display",
  "font-mono",
  "font-pane-title",
  "font-project",
  "type-meta",
  "type-small",
  "type-interface",
  "type-display-sm",
  "type-display-md",
  "type-display-lg",
  "type-mono-sm",
  "type-mono-cmd",
  "type-mono-md",
  "weight-body",
  "weight-label",
  "weight-heading",
  "space-2",
  "space-4",
  "space-6",
  "space-8",
  "space-10",
  "space-12",
  "space-14",
  "space-16",
  "space-24",
  "space-32",
  "space-48",
  "radius-2",
  "radius-4",
  "radius-5",
  "radius-6",
  "radius-7",
  "radius-8",
  "radius-10",
  "radius-12",
  "radius-16",
  "radius-full",
  "control-dense",
  "control-icon-small",
  "control-default",
  "control-menu",
  "control-large",
  "control-bar",
  "control-sidebar-top",
  "pulse-halo",
  "pulse-duration",
  "pulse-ease",
  "shadow-card",
  "app-pad",
  "app-gap",
  "region-radius",
  "crescent-band",
  "assistant-rule",
  "assistant-pad",
] as const;

/** The old names of global.css's palette that must keep resolving via an alias. */
const LEGACY_TOKENS = [
  "sand",
  "terracotta",
  "terracotta-deep",
  "terracotta-pressed",
  "terracotta-rgb",
  "selection",
  "green",
  "green-deep",
  "purple",
  "purple-deep",
  "ochre",
  "ochre-deep",
  "silence",
  "terminal-silence",
  "border",
  "border-strong",
  "surface",
  "surface-muted",
  "surface-muted-rgb",
  "danger-deep",
  "design-canvas",
  "design-grid",
  "line-subtle",
  "white",
  "ink-rgb",
] as const;

interface ParsedSheet {
  light: Map<string, string>;
  dark: Map<string, string>;
  aliasLight: Map<string, string>;
  aliasDark: Map<string, string>;
}

const ALIAS_MARKER = "Legacy aliases";

function parseBlock(block: string): Map<string, string> {
  const vars = new Map<string, string>();
  const line = /--([a-zA-Z0-9-]+)\s*:\s*([^;]+);/g;
  let match: RegExpExecArray | null;
  while ((match = line.exec(block)) !== null) {
    vars.set(`--${match[1]!}`, match[2]!.trim());
  }
  return vars;
}

function parseSheet(css: string): ParsedSheet {
  const markerIndex = css.indexOf(ALIAS_MARKER);
  expect(markerIndex, "the alias section marker is missing").toBeGreaterThan(0);
  const fresh = css.slice(0, markerIndex);
  const aliases = css.slice(markerIndex);

  const root = /:root\s*\{([^}]*)\}/g;
  const darkSel = /\[data-theme="dark"\]\s*\{([^}]*)\}/g;
  const freshRoots = [...fresh.matchAll(root)].map((m) => m[1]!);
  const freshDarks = [...fresh.matchAll(darkSel)].map((m) => m[1]!);
  const aliasRoots = [...aliases.matchAll(root)].map((m) => m[1]!);
  const aliasDarks = [...aliases.matchAll(darkSel)].map((m) => m[1]!);

  return {
    light: parseBlock(freshRoots.join("\n")),
    dark: parseBlock(freshDarks.join("\n")),
    aliasLight: parseBlock(aliasRoots.join("\n")),
    aliasDark: parseBlock(aliasDarks.join("\n")),
  };
}

const sheet = parseSheet(readFileSync(CSS_PATH, "utf8"));

describe("the new token blocks", () => {
  it("define every SPEC colour token in light, and re-declare each in dark", () => {
    for (const name of COLOUR_TOKENS) {
      expect(sheet.light.has(`--${name}`), `--${name} missing from :root`).toBe(true);
      expect(sheet.dark.has(`--${name}`), `--${name} missing from the dark block`).toBe(true);
    }
  });

  it("define the theme-independent tokens once, in :root", () => {
    for (const name of SHARED_TOKENS) {
      expect(sheet.light.has(`--${name}`), `--${name} missing from :root`).toBe(true);
      expect(sheet.dark.has(`--${name}`), `--${name} must stay theme-independent`).toBe(false);
    }
  });

  it("define --ground-app with the exact values theme.ts mirrors into the theme-color meta", () => {
    expect(sheet.light.get("--ground-app")?.toLowerCase()).toBe(GROUND_BY_THEME.light);
    expect(sheet.dark.get("--ground-app")?.toLowerCase()).toBe(GROUND_BY_THEME.dark);
  });

  it("never introduce a size below 12px", () => {
    for (const [name, value] of [...sheet.light, ...sheet.dark]) {
      const px = /^(?!.*var\()(\d+(?:\.\d+)?)px$/.exec(value);
      if (px === null || !name.startsWith("--type-")) continue;
      expect(Number.parseFloat(px[1]!), `${name} = ${value}`).toBeGreaterThanOrEqual(12);
    }
  });

  it("carry the one motion: the working pulse, static under reduced motion", () => {
    const css = readFileSync(CSS_PATH, "utf8");
    expect(css).toContain(".dot-pulse");
    expect(css).toContain("@keyframes breathe");
    const reduced = css.slice(css.indexOf("prefers-reduced-motion"));
    expect(reduced).toContain(".dot-pulse");
    expect(reduced).toMatch(/animation:\s*none/);
  });
});

describe("the legacy alias block", () => {
  it("gives every old token an alias, so today's stylesheets keep rendering", () => {
    const missing = LEGACY_TOKENS.filter(
      (name) => !sheet.aliasLight.has(`--${name}`) && !sheet.light.has(`--${name}`),
    );
    expect(
      missing,
      `old tokens without an alias: ${missing.map((n) => `--${n}`).join(", ")}`,
    ).toEqual([]);
  });

  it("never references an undefined token and never invents a hex colour", () => {
    const defined = new Set<string>([
      ...sheet.light.keys(),
      ...sheet.dark.keys(),
      ...sheet.aliasLight.keys(),
      ...sheet.aliasDark.keys(),
    ]);
    const problems: string[] = [];
    for (const [name, value] of [...sheet.aliasLight, ...sheet.aliasDark]) {
      if (/^[\d\s,]+$/.test(value)) continue; // rgb-triplet shims have no token form yet
      const refs = [...value.matchAll(/var\(--([a-zA-Z0-9-]+)/g)].map((m) => `--${m[1]}`);
      if (refs.length === 0) problems.push(`${name}: ${value} references no token`);
      for (const ref of refs) {
        if (!defined.has(ref)) problems.push(`${name}: ${value} references undefined ${ref}`);
      }
      if (/#[0-9a-fA-F]{3,8}/.test(value)) problems.push(`${name}: ${value} hardcodes a colour`);
    }
    expect(problems, problems.join("; ")).toEqual([]);
  });

  it("keeps the terracotta rgb shims matching their theme's accent", () => {
    const asTriplet = (hex: string): number[] => [
      parseInt(hex.slice(1, 3), 16),
      parseInt(hex.slice(3, 5), 16),
      parseInt(hex.slice(5, 7), 16),
    ];
    for (const [shim, accent] of [
      [sheet.aliasLight.get("--terracotta-rgb"), sheet.light.get("--accent")],
      [sheet.aliasDark.get("--terracotta-rgb"), sheet.dark.get("--accent")],
    ] as const) {
      expect(shim).toMatch(/^\d+,\s*\d+,\s*\d+$/);
      expect(accent).toMatch(/^#[0-9a-fA-F]{6}$/);
      expect(shim!.split(",").map((part) => Number.parseInt(part.trim(), 10))).toEqual(
        asTriplet(accent!),
      );
    }
  });
});

describe("the type ramp as the base", () => {
  it("gives body the interface size, so nothing falls back to the browser default", () => {
    const css = readFileSync(GLOBAL_CSS_PATH, "utf8");
    const body = /body\s*\{([^}]*)\}/.exec(css)?.[1];
    expect(body, "a body rule must exist in global.css").toBeTruthy();
    expect(body).toMatch(/font-size:\s*var\(--type-interface\)/);
  });
});
