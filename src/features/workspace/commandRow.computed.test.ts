// @vitest-environment happy-dom
// The command row's chrome through the real stylesheet: the mono chip on
// --code-bg (SPEC-regions "Command") and the exit dot + sentence, in both
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
    "builds the chip and the exit marker in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([
        "*",
        ".workspace-chat-tool-text",
        ".workspace-chat-tool-label",
        ".workspace-command-chip",
        ".workspace-command-dot",
        ".workspace-command-dot.is-failed",
        ".workspace-command-exit",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] .workspace-command-exit",
      ]);

      const summary = document.createElement("summary");
      const block = document.createElement("span");
      block.className = "workspace-chat-tool-text";
      const label = document.createElement("span");
      label.className = "workspace-chat-tool-label";
      label.textContent = "Shell";
      const chip = document.createElement("span");
      chip.className = "workspace-command-chip";
      chip.textContent = "pnpm vitest checkout";
      block.append(label, chip);
      const dotOk = document.createElement("span");
      dotOk.className = "workspace-command-dot";
      const dotFail = document.createElement("span");
      dotFail.className = "workspace-command-dot is-failed";
      const exitText = document.createElement("span");
      exitText.className = "workspace-command-exit";
      exitText.textContent = "exit 1";
      summary.append(block, dotOk, dotFail, exitText);
      document.body.append(summary);

      const chipStyle = getComputedStyle(chip);
      expect(chipStyle.backgroundColor).toBe(css.token("--code-bg"));
      expect(chipStyle.color).toBe(css.token("--code-text"));
      expect(chipStyle.borderRadius).toBe("4px");
      // A decorative boundary in the copyable block's form (SPEC-regions:79);
      // the mono text is what identifies the chip, so the contrast lives there.
      expect(chipStyle.borderTopWidth).toBe("1px");
      expect(chipStyle.borderTopColor).toBe(css.token("--line-strong"));
      const fillTool = css.token("--fill-tool");
      const codeBg = css.token("--code-bg");
      expect(fillTool).toBeDefined();
      expect(codeBg).toBeDefined();
      expect(contrastRatio(chipStyle.color, codeBg!)).toBeGreaterThanOrEqual(4.5);
      expect(chipStyle.fontFamily).toContain("JetBrains Mono");
      expect(chipStyle.fontSize).toBe("12px");
      // Border-box accounting: 22 − 2 (border) keeps the 20 px line box whole,
      // and 6 + 1 keeps the horizontal inset at the mockup's 7.
      expect(chipStyle.boxSizing).toBe("border-box");
      expect(chipStyle.height).toBe("22px");
      expect(chipStyle.paddingLeft).toBe("6px");
      expect(chipStyle.textOverflow).toBe("ellipsis");
      expect(chipStyle.whiteSpace).toBe("nowrap");
      expect(chipStyle.overflow).toBe("hidden");
      expect(chipStyle.minWidth).toBe("0");
      // The chip is the yielding text of a command row, like the summary it replaces.
      expect(Number.parseFloat(chipStyle.flexShrink)).toBeGreaterThan(1);

      const dotOkStyle = getComputedStyle(dotOk);
      expect(dotOkStyle.backgroundColor).toBe(css.token("--tone-live"));
      expect(dotOkStyle.width).toBe("6px");
      expect(dotOkStyle.height).toBe("6px");
      expect(dotOkStyle.borderRadius).toBe("999px");
      expect(contrastRatio(dotOkStyle.backgroundColor, fillTool!)).toBeGreaterThanOrEqual(3);
      const dotFailStyle = getComputedStyle(dotFail);
      expect(dotFailStyle.backgroundColor).toBe(css.token("--danger"));
      expect(contrastRatio(dotFailStyle.backgroundColor, fillTool!)).toBeGreaterThanOrEqual(3);

      const exitStyle = getComputedStyle(exitText);
      expect(exitStyle.color).toBe(css.token("--muted"));
      expect(exitStyle.fontSize).toBe("12px");
      expect(exitStyle.whiteSpace).toBe("nowrap");
      expect(exitStyle.flexShrink).toBe("0");
      expect(contrastRatio(exitStyle.color, fillTool!)).toBeGreaterThanOrEqual(4.5);

      summary.remove();
    },
  );

  it.each(["light", "dark"] as const)(
    "keeps the exit marker readable on the open row's ground in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      // Every rule the sheet writes for these classes, in any selector form:
      // a future [open] rule must reach the computed style or this pin is dead.
      const targets = css.rules
        .flatMap((rule) => rule.selector.split(","))
        .map((selector) => selector.trim())
        .filter(
          (selector) =>
            selector.includes(".workspace-command-dot") ||
            selector.includes(".workspace-command-exit"),
        );
      css.inject(targets);

      const openRow = document.createElement("details");
      openRow.className = "workspace-chat-entry workspace-chat-tool";
      openRow.open = true;
      const openSummary = document.createElement("summary");
      const openDotOk = document.createElement("span");
      openDotOk.className = "workspace-command-dot";
      const openDotFail = document.createElement("span");
      openDotFail.className = "workspace-command-dot is-failed";
      const openExit = document.createElement("span");
      openExit.className = "workspace-command-exit";
      openExit.textContent = "exit 1";
      openSummary.append(openDotOk, openDotFail, openExit);
      openRow.append(openSummary);
      document.body.append(openRow);

      const codeBg = css.token("--code-bg");
      expect(codeBg).toBeDefined();
      const dotFail = getComputedStyle(openDotFail);
      expect(dotFail.backgroundColor).toBe(css.token("--diff-del"));
      expect(contrastRatio(dotFail.backgroundColor, codeBg!)).toBeGreaterThanOrEqual(3);
      const dotOk = getComputedStyle(openDotOk);
      expect(dotOk.backgroundColor).toBe(css.token("--tone-live"));
      expect(contrastRatio(dotOk.backgroundColor, codeBg!)).toBeGreaterThanOrEqual(3);
      // Equality, not just a ratio: --muted clears 4.5:1 on this ground in the
      // dark theme, so a ratio alone would let a broken recolour pass there.
      const exitStyle = getComputedStyle(openExit);
      expect(exitStyle.color).toBe(css.token("--code-text"));
      expect(contrastRatio(exitStyle.color, codeBg!)).toBeGreaterThanOrEqual(4.5);

      openRow.remove();
    },
  );
});
