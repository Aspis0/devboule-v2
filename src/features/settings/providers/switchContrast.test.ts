// The provider switch's on fill must read apart from its off track in both
// themes. Read from the real token file: the first `:root` block is the light
// theme, the first `[data-theme="dark"]` block is the dark one.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const tokens = readFileSync(resolve(import.meta.dirname, "../../../styles/tokens.css"), "utf8");

function block(selector: RegExp): string {
  const match = tokens.match(selector);
  if (!match) throw new Error(`no block matching ${selector}`);
  return match[1];
}

function hexOf(theme: string, name: string): string {
  const value = theme.match(new RegExp(`${name}:\\s*(#[0-9a-fA-F]{6})\\s*;`))?.[1];
  if (!value) throw new Error(`${name} is not a hex colour in this theme`);
  return value;
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((index) => {
    const channel = parseInt(hex.slice(index, index + 2), 16) / 255;
    return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(first: string, second: string): number {
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

describe("provider switch on and off fills in both themes", () => {
  const themes = {
    light: block(/:root\s*\{([\s\S]*?)\n\}/),
    dark: block(/\[data-theme="dark"\]\s*\{([\s\S]*?)\n\}/),
  };

  for (const [name, theme] of Object.entries(themes)) {
    it(`separates the on fill from the off track at 3:1 or more in ${name} theme`, () => {
      const on = hexOf(theme, "--tone-live");
      const off = hexOf(theme, "--line-strong");
      expect(contrast(on, off)).toBeGreaterThanOrEqual(3);
    });
  }
});
