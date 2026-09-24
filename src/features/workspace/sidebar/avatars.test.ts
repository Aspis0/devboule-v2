import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { avatarStyle, avatarTone, type AvatarTone } from "./avatars";

describe("avatar tones", () => {
  it("pick one of the mockup's five tones, deterministically from the id", () => {
    const tones = new Set<AvatarTone>();
    for (let index = 0; index < 200; index += 1) {
      const id = `workspace-${index}`;
      expect(avatarTone(id)).toBe(avatarTone(id));
      tones.add(avatarTone(id));
    }
    // The whole palette is in use: the ids spread across all five tones.
    expect(tones.size).toBe(5);
  });

  it("always returns one of the five tones for arbitrary ids", () => {
    for (const id of ["", "a", "(devboule)", "workspace:8F3A-2", "🔌"]) {
      expect(["live", "recovered", "attention", "unattended", "idle"]).toContain(avatarTone(id));
    }
  });

  it("mix the tone over transparent at the mockup's percentages", () => {
    const style = avatarStyle(avatarTone("workspace-1") === "live" ? "workspace-1" : "x");
    expect(style.background).toMatch(
      /color-mix\(in srgb, var\(--tone-[a-z]+\) (20|22|25)%, transparent\)/,
    );
    expect(style.color).toMatch(/var\(--tone-[a-z]+\)/);
  });

  it("keeps the tone identical for the same id everywhere it renders", () => {
    expect(avatarStyle("project-9")).toEqual(avatarStyle("project-9"));
    expect(avatarTone("project-9")).toBe(avatarTone("project-9"));
  });
});

describe("avatar letter contrast (computed from tokens.css)", () => {
  // sRGB lerp stands in for color-mix(in srgb), alpha compositing for the
  // transparent background over --panel-side — the same math the browser
  // applies, so the ratios here are the browser's ratios.
  const css = readFileSync(resolve(import.meta.dirname, "../../../styles/tokens.css"), "utf8");

  function blockVars(selector: string): Map<string, string> {
    const at = css.indexOf(selector);
    if (at < 0) throw new Error(`selector ${selector} not found in tokens.css`);
    const open = css.indexOf("{", at);
    const close = css.indexOf("}", open);
    const vars = new Map<string, string>();
    for (const m of css.slice(open + 1, close).matchAll(/--([a-z-]+):\s*([^;]+);/g)) {
      vars.set(m[1]!.trim(), m[2]!.trim());
    }
    return vars;
  }

  function hexToRgb(hex: string): [number, number, number] {
    return [
      parseInt(hex.slice(1, 3), 16),
      parseInt(hex.slice(3, 5), 16),
      parseInt(hex.slice(5, 7), 16),
    ];
  }

  const MIX: Record<AvatarTone, number> = {
    live: 20,
    recovered: 22,
    attention: 20,
    unattended: 20,
    idle: 25,
  };
  const TONES: AvatarTone[] = ["live", "recovered", "attention", "unattended", "idle"];

  for (const [theme, selector] of [
    ["light", ":root"],
    ["dark", '[data-theme="dark"]'],
  ] as const) {
    const vars = blockVars(selector);
    const ink = hexToRgb(vars.get("ink")!);
    const panel = hexToRgb(vars.get("panel-side")!);

    for (const tone of TONES) {
      it(`${theme}: ${tone} avatar letter ≥ 4.5:1 on its own background`, () => {
        const toneRgb = hexToRgb(vars.get(`tone-${tone}`)!);
        const background = toneRgb.map(
          (channel, i) =>
            channel * (MIX[tone as AvatarTone] / 100) +
            panel[i] * (1 - MIX[tone as AvatarTone] / 100),
        );
        // The letter colour is read from what the implementation actually
        // declares for this tone — a regression to the raw tone (0% mix) or
        // any weaker mix fails here, not only in the browser.
        const style = avatarStyle(`${theme}-${tone}`);
        const colour = String(style.color);
        const colourMatch = /color-mix\(in srgb, var\(--tone-[a-z]+\) (\d+)%, var\(--ink\)\)/.exec(
          colour,
        );
        expect(
          colourMatch,
          `avatarStyle colour must mix the tone into --ink (got: ${colour})`,
        ).not.toBeNull();
        const letterMix = Number(colourMatch![1]!);
        const letter = toneRgb.map(
          (channel, i) => channel * (letterMix / 100) + ink[i] * (1 - letterMix / 100),
        );
        const lum = ([r, g, b]: number[]) => {
          const f = (c: number) => {
            c /= 255;
            return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
          };
          return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
        };
        const l1 = lum(letter);
        const l2 = lum(background);
        const ratio = (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
        expect(ratio, `${theme}/${tone} ratio ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
});
