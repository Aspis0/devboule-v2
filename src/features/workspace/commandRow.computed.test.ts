// @vitest-environment happy-dom
// The command row's chrome through the real stylesheet: the command as plain
// mono text, the exit mark and sentence, on the transcript ground in both
// themes — computed styles only, never the rule text.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof, type CssTheme } from "./cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");
const sheets = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/styles/global.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/Workspace.css"), "utf8"),
];

function luminance(color: string): number {
  const hex = color.match(/^#([\da-f]{6})$/i)?.[1];
  const channels =
    hex !== undefined
      ? [0, 2, 4].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255)
      : color
          .match(/[\d.]+/g)
          ?.slice(0, 3)
          .map((channel) => Number(channel) / 255);
  if (channels === undefined || channels.length !== 3) {
    throw new Error(`Expected an RGB color, received ${color}`);
  }
  const linear = channels.map((channel) =>
    channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return linear[0]! * 0.2126 + linear[1]! * 0.7152 + linear[2]! * 0.0722;
}

function contrastRatio(foreground: string, background: string): number {
  const [lighter, darker] =
    luminance(foreground) > luminance(background)
      ? [foreground, background]
      : [background, foreground];
  return (luminance(lighter) + 0.05) / (luminance(darker) + 0.05);
}

afterEach(() => {
  removeCssProof();
  document.documentElement.removeAttribute("data-theme");
});

describe("command row computed styles", () => {
  it.each(["light", "dark"] as const)(
    "draws the command and the exit marker as plain text in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([
        "*",
        ".workspace-chat-tool",
        ".workspace-chat-tool-text",
        ".workspace-command-chip",
        ".workspace-command-dot",
        ".workspace-command-exit",
      ]);

      const row = document.createElement("div");
      row.className = "workspace-chat-entry workspace-chat-tool";
      const summary = document.createElement("div");
      const block = document.createElement("span");
      block.className = "workspace-chat-tool-text";
      const chip = document.createElement("span");
      chip.className = "workspace-command-chip";
      chip.textContent = "pnpm vitest checkout";
      block.append(chip);
      // A line carries only a non-zero exit, so its one mark is the failure mark.
      const dot = document.createElement("span");
      dot.className = "workspace-command-dot";
      const exitText = document.createElement("span");
      exitText.className = "workspace-command-exit";
      exitText.textContent = "exit 1";
      summary.append(block, dot, exitText);
      row.append(summary);
      document.body.append(row);

      const ground = css.token("--ground-center");
      expect(ground).toBeDefined();

      // The command is text on the line: no fill, no border, no box of its own.
      const chipStyle = getComputedStyle(chip);
      expect(["", "transparent", "rgba(0, 0, 0, 0)"]).toContain(chipStyle.backgroundColor);
      expect(["", "0px"]).toContain(chipStyle.borderTopWidth);
      expect(chipStyle.color).toBe(css.token("--muted"));
      expect(contrastRatio(chipStyle.color, ground!)).toBeGreaterThanOrEqual(4.5);
      expect(getComputedStyle(row).fontFamily).toContain("Inter");
      expect(chipStyle.textOverflow).toBe("ellipsis");
      expect(chipStyle.whiteSpace).toBe("nowrap");
      expect(chipStyle.overflow).toBe("hidden");
      expect(chipStyle.minWidth).toBe("12ch");

      // The mark is a glyph in the danger colour; the sentence is what carries the code.
      const dotStyle = getComputedStyle(dot);
      expect(dotStyle.color).toBe(css.token("--danger"));
      expect(["", "transparent", "rgba(0, 0, 0, 0)"]).toContain(dotStyle.backgroundColor);
      expect(contrastRatio(dotStyle.color, ground!)).toBeGreaterThanOrEqual(3);

      const exitStyle = getComputedStyle(exitText);
      expect(exitStyle.color).toBe(css.token("--muted"));
      expect(exitStyle.whiteSpace).toBe("nowrap");
      expect(exitStyle.flexShrink).toBe("0");
      expect(contrastRatio(exitStyle.color, ground!)).toBeGreaterThanOrEqual(4.5);

      row.remove();
    },
  );
});
