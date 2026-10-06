// The focus ring on the grounds it is really drawn on. palette-contrast.test.ts
// holds each (ring token, ground token) pair to 3:1; this walk reads every
// stylesheet and proves the CSS uses those pairs:
//   1. a rule that paints a ground that is dark in both themes (--code-bg,
//      --terminal-ground) sets `--ring` to --accent-on-code, or is allow-listed
//      as having no focusable control to ring;
//   2. every `--ring` a rule sets reaches 3:1 on that rule's ground;
//   3. a rule that paints a ground and also draws an accent outline or edge on
//      it uses a pair that reaches 3:1, in both themes;
//   4. the selectors that draw their outline on a dark scope's descendants read
//      `--ring`, never the accent directly.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { contrastRatio, HEX_COLOR } from "./contrast";
import { resolveVars } from "./cssText";
import {
  find,
  groundOf,
  hex,
  label,
  RULES,
  THEMES,
  tokens,
  varName,
  type SheetRule,
  type Theme,
} from "./sheetRules";

const DARK_GROUNDS = new Set(["--code-bg", "--terminal-ground"]);
const ON_DARK_RING = "var(--accent-on-code)";
const RING_FLOOR = 3;

/** The accent and every alias that resolves onto it, plus the ring tokens. */
function ringFamily(): Set<string> {
  const family = new Set<string>(["--accent", "--accent-on-code", "--ring"]);
  for (const theme of THEMES) {
    const withoutAccent = new Map(tokens[theme]);
    withoutAccent.delete("--accent");
    for (const [name, value] of tokens[theme]) {
      if (resolveVars(value, withoutAccent).text.trim() === "var(--accent)") family.add(name);
    }
  }
  return family;
}

const FAMILY = ringFamily();

function ratio(ringToken: string, groundToken: string, theme: Theme): number {
  return contrastRatio(hex(ringToken, theme), hex(groundToken, theme));
}

/** Rules that paint a dark-in-both-themes ground and hold no control a ring is
 * drawn over, so they need no `--ring`. Each one says why. */
const RINGLESS: ReadonlyArray<{ file: string; selector: string; why: string }> = [
  {
    file: "src/components/codeBlocks.css",
    selector: ".codeblock-sample pre",
    why: "its copy button is a sibling, ringed by .codeblock-sample",
  },
  {
    file: "src/features/workspace/Workspace.css",
    selector: ".workspace-terminal-host",
    why: "inside .workspace-terminal-ground, which sets the ring",
  },
  {
    file: "src/features/workspace/Workspace.css",
    selector: ".workspace-terminal-host .xterm .xterm-viewport",
    why: "inside .workspace-terminal-ground, which sets the ring",
  },
  {
    file: "src/features/workspace/Workspace.css",
    selector: ".workspace-terminal-host .xterm .xterm-viewport::-webkit-scrollbar-track",
    why: "a scrollbar track, not a control",
  },
  {
    file: "src/features/workspace/Workspace.css",
    selector:
      ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] .workspace-chat-tool-interrupted",
    why: "a text marker inside the open row, which sets the ring",
  },
  {
    file: "src/features/workspace/Workspace.css",
    selector: ".workspace-command-chip",
    why: "a non-focusable text chip",
  },
];

/** A rule that sets `--ring` over a ground it does not paint itself. */
const SCOPE_GROUNDS: ReadonlyArray<{ file: string; selector: string; ground: string }> = [
  {
    file: "src/components/codeBlocks.css",
    selector: ".codeblock-sample",
    ground: "--code-bg",
  },
];

/** Selectors that draw an outline on whatever scope they sit in, so the colour
 * has to come from `--ring`. */
const RING_READERS: ReadonlyArray<{ file: string; selector: string }> = [
  { file: "src/styles/global.css", selector: "button:focus-visible" },
  {
    file: "src/features/workspace/Workspace.css",
    selector:
      ".workspace-chat-tool summary:focus-visible, .workspace-chat-tool-group summary:focus-visible",
  },
  { file: "src/components/codeBlocks.css", selector: ".copy-btn:focus-visible" },
];

/** The ring-like paint a rule draws: an outline or a thin edge. */
const RING_PROPERTIES =
  /^(outline|outline-color|border-left|border-left-color|border-inline-start)$/;

function ringTokensDrawn(rule: SheetRule): string[] {
  const drawn: string[] = [];
  for (const [property, value] of rule.declarations) {
    if (!RING_PROPERTIES.test(property)) continue;
    for (const match of value.matchAll(/var\(\s*(--[a-zA-Z0-9-]+)/g)) {
      if (FAMILY.has(match[1]!)) drawn.push(match[1]!);
    }
  }
  return drawn;
}

describe("the focus ring on the grounds the stylesheets paint", () => {
  it("walked the stylesheets", () => {
    expect(RULES.length).toBeGreaterThan(500);
    expect(RULES.some((rule) => groundOf(rule) === "--code-bg")).toBe(true);
  });

  it("every rule painting a dark-in-both-themes ground sets --ring to the on-dark accent", () => {
    const missing = RULES.filter((rule) => DARK_GROUNDS.has(groundOf(rule) ?? ""))
      .filter((rule) => rule.declarations.get("--ring") !== ON_DARK_RING)
      .filter((rule) => !RINGLESS.some((a) => a.file === rule.file && a.selector === rule.selector))
      .map(label);
    expect(missing).toEqual([]);
  });

  it("holds the ringless allow-list to rules that still exist", () => {
    const stale = RINGLESS.filter((a) => find(a.file, a.selector) === undefined);
    expect(stale.map((a) => `${a.file}: ${a.selector}`)).toEqual([]);
  });

  it("every --ring a rule sets reaches 3:1 on that rule's ground, in both themes", () => {
    const scoped = RULES.filter((rule) => rule.declarations.has("--ring"));
    expect(scoped.length).toBeGreaterThan(0);
    const failures: string[] = [];
    for (const rule of scoped) {
      const ring = varName(rule.declarations.get("--ring"));
      const ground =
        groundOf(rule) ??
        SCOPE_GROUNDS.find((s) => s.file === rule.file && s.selector === rule.selector)?.ground;
      if (ring === null || ground === undefined) {
        failures.push(`${label(rule)}: --ring needs a var() value and a known ground`);
        continue;
      }
      for (const theme of THEMES) {
        const value = ratio(ring, ground, theme);
        if (value < RING_FLOOR) {
          failures.push(`${label(rule)} (${theme}): ${ring} on ${ground} is ${value.toFixed(2)}`);
        }
      }
    }
    expect(failures).toEqual([]);
  });

  it("every outline or edge a rule draws on its own ground reaches 3:1, in both themes", () => {
    const failures: string[] = [];
    let checked = 0;
    for (const rule of RULES) {
      const ground = groundOf(rule);
      if (ground === null || FAMILY.has(ground)) continue;
      for (const drawn of ringTokensDrawn(rule)) {
        const own = varName(rule.declarations.get("--ring"));
        const ring = drawn === "--ring" && own !== null ? own : drawn;
        for (const theme of THEMES) {
          if (!HEX_COLOR.test(resolveVars(`var(${ground})`, tokens[theme]).text.trim())) continue;
          checked += 1;
          const value = ratio(ring, ground, theme);
          if (value < RING_FLOOR) {
            failures.push(`${label(rule)} (${theme}): ${ring} on ${ground} is ${value.toFixed(2)}`);
          }
        }
      }
    }
    expect(checked).toBeGreaterThan(0);
    expect(failures).toEqual([]);
  });

  it("the outlines drawn over a dark scope's descendants read --ring", () => {
    const problems: string[] = [];
    for (const reader of RING_READERS) {
      const rule = find(reader.file, reader.selector);
      if (rule === undefined) {
        problems.push(`${reader.file}: ${reader.selector} no longer exists`);
        continue;
      }
      if (!(rule.declarations.get("outline") ?? "").includes("var(--ring)")) {
        problems.push(`${label(rule)}: outline does not read var(--ring)`);
      }
    }
    expect(problems).toEqual([]);
  });
});
