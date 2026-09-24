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
  // transparent background over --panel-side and --fill-selected — the same
  // math the browser applies, so the ratios here are the browser's ratios.
  const css = readFileSync(resolve(import.meta.dirname, "../../../styles/tokens.css"), "utf8");
  // Comments are stripped BEFORE block matching: the header comment names
  // [data-theme="dark"], and a naive indexOf would match it and read the
  // light :root block for the dark theme (the pass-2 review defect).
  const bareCss = css.replace(/\/\*[\s\S]*?\*\//g, "");

  function blockVars(selector: string): Map<string, string> {
    const at = bareCss.indexOf(selector);
    if (at < 0) throw new Error(`selector ${selector} not found in tokens.css`);
    const open = bareCss.indexOf("{", at);
    const close = bareCss.indexOf("}", open);
    const vars = new Map<string, string>();
    for (const m of bareCss.slice(open + 1, close).matchAll(/--([a-z-]+):\s*([^;]+);/g)) {
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

  // The dark block's ink differs from the light block's: proves blockVars
  // reads THIS theme's block (comments are stripped before matching, so the
  // header comment's [data-theme="dark"] mention cannot hijack the search).
  const lightInk = blockVars(":root").get("ink")!;
  const darkInk = blockVars('[data-theme="dark"]').get("ink")!;
  for (const [theme, selector] of [
    ["light", ":root"],
    ["dark", '[data-theme="dark"]'],
  ] as const) {
    const vars = blockVars(selector);
    const ink = hexToRgb(vars.get("ink")!);
    const panel = hexToRgb(vars.get("panel-side")!);
    const fillSelected = hexToRgb(vars.get("fill-selected")!);

    it(`${theme}: blockVars reads the ${theme} ink (dark differs from light)`, () => {
      expect(vars.get("ink")).toBe(theme === "dark" ? darkInk : lightInk);
      expect(lightInk).not.toBe(darkInk);
    });

    for (const tone of TONES) {
      for (const [groundName, ground] of [
        ["panel-side", panel],
        ["fill-selected", fillSelected],
      ] as const) {
        it(`${theme}: ${tone} avatar letter >= 4.5:1 on ${groundName} (${ground})`, () => {
          const toneRgb = hexToRgb(vars.get(`tone-${tone}`)!);
          // The avatar's own background: the tone tinted over the ground the
          // row paints (panel-side normally, fill-selected when selected).
          const background = toneRgb.map(
            (channel, i) =>
              channel * (MIX[tone as AvatarTone] / 100) +
              ground[i] * (1 - MIX[tone as AvatarTone] / 100),
          );
          // The letter colour is read from what the implementation actually
          // declares for this tone: it must name the tone itself and take
          // its mix strength from the --avatar-letter-mix token. A
          // regression to the raw tone, a weaker mix, or a deleted token
          // fails here, not only in the browser.
          const id = `${theme}-${tone}`;
          const style = avatarStyle(id);
          const colour = String(style.color);
          const colourMatch =
            /color-mix\(in srgb, var\(--tone-([a-z]+)\) var\(--avatar-letter-mix\), var\(--ink\)\)/.exec(
              colour,
            );
          expect(
            colourMatch,
            `avatarStyle colour must mix the tone into --ink via --avatar-letter-mix (got: ${colour})`,
          ).not.toBeNull();
          expect(
            colourMatch![1]!,
            `avatarStyle must use this avatar's own tone (got ${colourMatch![1]!}, wanted ${avatarTone(id)})`,
          ).toBe(avatarTone(id));
          const letterMix = Number(blockVars(":root").get("avatar-letter-mix")!.replace("%", ""));
          expect(letterMix).toBeGreaterThanOrEqual(30);
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
  }
});
