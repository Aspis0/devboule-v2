// @vitest-environment happy-dom

// The strip's two kinds of tab through the real stylesheets: agents in ink at
// the full label size, tools muted at the smaller regular size and still above
// the contrast floor, and the one marker on whichever tab is in front.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");
const sheets = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/strip/strip.css"), "utf8"),
];

afterEach(() => {
  removeCssProof();
  document.documentElement.removeAttribute("data-theme");
});

function luminance(color: string): number {
  const hex = color.match(/^#([\da-f]{6})$/i)?.[1];
  const channels =
    hex === undefined
      ? color
          .match(/[\d.]+/g)
          ?.slice(0, 3)
          .map((channel) => Number(channel) / 255)
      : [0, 2, 4].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255);
  if (channels === undefined || channels.length !== 3) {
    throw new Error(`Expected an RGB color, received ${color}`);
  }
  const [r, g, b] = channels.map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return r! * 0.2126 + g! * 0.7152 + b! * 0.0722;
}

function contrast(foreground: string, background: string): number {
  const [a, b] = [luminance(foreground), luminance(background)];
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function tab(className: string): { button: HTMLElement; label: HTMLElement } {
  const button = document.createElement("button");
  button.className = `workspace-session-tab ${className}`;
  const label = document.createElement("span");
  label.className = "workspace-tab-label";
  button.append(label);
  document.body.append(button);
  return { button, label };
}

describe("agent and tool tabs", () => {
  it("keeps the marker clear of the close scrim, which starts under the tab's top edge", () => {
    const css = assembleCssProof(sheets, "light");
    expect(css.rulesFor(".workspace-session-chip::before")).toContain("inset: 2px 0 0");
    // The reveal rules only repaint the scrim: none of them moves its edge back to the top.
    const reveals = css.rules.filter(
      (rule) =>
        rule.selector.includes(".workspace-session-chip::before") && !/inset|top/.test(rule.body),
    );
    expect(reveals.length).toBeGreaterThan(0);
    for (const hovered of [
      ".workspace-session-tab-selected:hover",
      ".workspace-session-tab-selected:focus-visible",
    ]) {
      expect(css.rulesFor(hovered)).not.toContain("box-shadow");
    }
  });

  it.each(["light", "dark"] as const)(
    "keeps a multi-selected quiet tab above the contrast floor in the %s theme",
    (theme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([".workspace-session-tab", ".workspace-session-tab-multiselected"]);
      const { button } = tab("workspace-session-tab-quiet workspace-session-tab-multiselected");
      const style = getComputedStyle(button);

      expect(style.backgroundColor).toBe(css.token("--fill-selected-soft"));
      expect(contrast(style.color, style.backgroundColor)).toBeGreaterThanOrEqual(6.97);
    },
  );

  it.each(["light", "dark"] as const)(
    "reads an agent in ink at 14px and a tool muted at 12px regular, in the %s theme",
    (theme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([
        ".workspace-session-tab",
        ".workspace-session-tab-agent",
        ".workspace-tab-label",
        ".workspace-session-tab-quiet .workspace-tab-label",
      ]);
      const agent = tab("workspace-session-tab-agent");
      const tool = tab("workspace-session-tab-quiet");

      expect(getComputedStyle(agent.button).color).toBe(css.token("--ink"));
      expect(getComputedStyle(agent.label).fontSize).toBe("14px");
      expect(getComputedStyle(agent.label).fontWeight).toBe("500");
      expect(getComputedStyle(tool.button).color).toBe(css.token("--muted"));
      expect(getComputedStyle(tool.label).fontSize).toBe("12px");
      expect(getComputedStyle(tool.label).fontWeight).toBe("400");
      // A quiet tab is lighter, never shorter.
      expect(getComputedStyle(tool.button).height).toBe(getComputedStyle(agent.button).height);
      expect(
        contrast(getComputedStyle(tool.button).color, css.token("--ground-center")!),
      ).toBeGreaterThanOrEqual(6.97);
    },
  );

  it.each(["light", "dark"] as const)(
    "marks the tab in front with a thin ochre edge over a neutral fill, of either kind, in the %s theme",
    (theme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([
        ".workspace-session-tab",
        ".workspace-session-tab-agent",
        ".workspace-session-tab-selected",
      ]);
      const agent = tab("workspace-session-tab-agent workspace-session-tab-selected");
      const tool = tab("workspace-session-tab-quiet workspace-session-tab-selected");

      for (const { button } of [agent, tool]) {
        const style = getComputedStyle(button);
        expect(style.backgroundColor).toBe(css.token("--fill-selected"));
        expect(style.backgroundColor).not.toBe(css.token("--accent"));
        expect(style.boxShadow).toBe(`inset 0 2px 0 ${css.token("--accent")}`);
        expect(style.color).toBe(css.token("--ink"));
      }
    },
  );
});
