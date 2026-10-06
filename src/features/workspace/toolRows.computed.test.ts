// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof, type CssTheme } from "./cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");
const sheets = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
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

/** happy-dom answers an unset paint property with the empty string. */
function unpainted(value: string): boolean {
  return value === "" || value === "transparent" || value === "rgba(0, 0, 0, 0)";
}

function noWidth(value: string): boolean {
  return value === "" || value === "0px";
}

function el(tag: string, className: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** Every selector the sheet writes about a tool row, so a later rule that
 * reaches these elements reaches the computed style these tests read. */
function injectToolRules(css: ReturnType<typeof assembleCssProof>): void {
  css.inject([
    "*",
    ...css.rules
      .flatMap((rule) => rule.selector.split(","))
      .map((selector) => selector.trim())
      .filter(
        (selector) =>
          selector.includes(".workspace-chat-tool") || selector.includes(".workspace-command"),
      ),
  ]);
}

describe("tool row computed styles", () => {
  it.each(["light", "dark"] as const)(
    "draws a tool call as one flat line of text on the transcript ground in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      injectToolRules(css);

      const row = el("div", "workspace-chat-entry workspace-chat-tool");
      const summary = el("div", "workspace-chat-tool-summary");
      const text = el("span", "workspace-chat-tool-text has-summary");
      const label = el("span", "workspace-chat-tool-label", "Edited");
      const target = el("span", "workspace-chat-tool-summary-text", "src/summary.ts");
      const stat = el("span", "workspace-chat-tool-stat", "(+12 −3)");
      text.append(label, target, stat);
      const done = el("span", "workspace-chat-tool-done", "✓");
      summary.append(text, done);
      row.append(summary);
      document.body.append(row);

      const ground = css.token("--ground-center");
      expect(ground).toBeDefined();
      // No fill, no border: a line is text, not a container.
      const rowStyle = getComputedStyle(row);
      expect(unpainted(rowStyle.backgroundColor)).toBe(true);
      expect(noWidth(rowStyle.borderTopWidth)).toBe(true);
      expect(unpainted(getComputedStyle(summary).backgroundColor)).toBe(true);
      // One font, two greys: the verb in the ink, everything else muted.
      expect(rowStyle.fontFamily).toContain("JetBrains Mono");
      expect(getComputedStyle(label).color).toBe(css.token("--ink"));
      for (const muted of [target, stat]) {
        expect(getComputedStyle(muted).color).toBe(css.token("--muted"));
      }
      // The floors the palette sets: ink 7 on the transcript ground, muted 6.97.
      expect(contrastRatio(getComputedStyle(label).color, ground!)).toBeGreaterThanOrEqual(7);
      for (const muted of [target, stat]) {
        expect(contrastRatio(getComputedStyle(muted).color, ground!)).toBeGreaterThanOrEqual(6.97);
      }
      // A long tool name ends in an ellipsis at a width of its own, and the target
      // keeps a share of the line whatever the name does.
      const labelStyle = getComputedStyle(label);
      expect(labelStyle.maxWidth).toBe("28ch");
      expect(labelStyle.textOverflow).toBe("ellipsis");
      expect(labelStyle.overflow).toBe("hidden");
      expect(labelStyle.minWidth).toBe("0");
      expect(getComputedStyle(target).textOverflow).toBe("ellipsis");
      expect(getComputedStyle(target).minWidth).toBe("12ch");
      expect(getComputedStyle(text).overflow).toBe("hidden");

      row.remove();
    },
  );

  it.each(["light", "dark"] as const)(
    "keeps the line's other text and marks readable on the transcript ground in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      injectToolRules(css);

      const row = el("div", "workspace-chat-entry workspace-chat-tool is-interrupted");
      const summary = el("div", "workspace-chat-tool-summary");
      const interrupted = el("span", "workspace-chat-tool-interrupted", "Interrupted");
      const location = el("span", "workspace-chat-tool-location", "src/main.ts");
      const exit = el("span", "workspace-command-exit", "exit 1");
      const done = el("span", "workspace-chat-tool-done", "✓");
      const running = el("span", "workspace-chat-tool-running");
      const failed = el("span", "workspace-chat-tool-failed", "failed");
      summary.append(interrupted, location, exit, done, running, failed);
      const group = el(
        "details",
        "workspace-chat-entry workspace-chat-tool workspace-chat-tool-group",
      );
      const count = el("span", "workspace-chat-tool-group-count", "3 tool calls");
      const groupText = el("span", "workspace-chat-tool-group-summary-text", "Ran 1 command");
      group.append(el("summary", "workspace-chat-tool-group-summary"), count, groupText);
      row.append(summary);
      document.body.append(row, group);

      const ground = css.token("--ground-center")!;
      // Text carries the palette's floors; a glyph or a dot only needs to be seen.
      for (const text of [interrupted, location, exit, groupText]) {
        expect(
          contrastRatio(getComputedStyle(text).color, ground),
          text.className,
        ).toBeGreaterThanOrEqual(6.97);
      }
      for (const text of [count, failed]) {
        expect(
          contrastRatio(getComputedStyle(text).color, ground),
          text.className,
        ).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrastRatio(getComputedStyle(done).color, ground)).toBeGreaterThanOrEqual(3);
      expect(
        contrastRatio(getComputedStyle(running).backgroundColor, ground),
      ).toBeGreaterThanOrEqual(3);
      // An interrupted line keeps its verb at full strength: only the state is said in words.
      expect(
        css.rulesFor(".workspace-chat-tool.is-interrupted .workspace-chat-tool-label"),
      ).toContain("opacity: 1");

      row.remove();
      group.remove();
    },
  );

  it.each(["light", "dark"] as const)(
    "boxes only a failure's excerpt, in the danger tone, in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      injectToolRules(css);

      const row = el("div", "workspace-chat-entry workspace-chat-tool is-failed");
      const plain = el("div", "workspace-chat-tool-output is-plain");
      const excerpt = el("div", "workspace-chat-tool-output is-failure");
      const lines = el("div", "workspace-chat-tool-output-lines");
      const more = el("button", "workspace-chat-tool-more", "+3 lines");
      excerpt.append(lines, more);
      const failed = el("span", "workspace-chat-tool-failed", "failed");
      row.append(plain, excerpt, failed);
      document.body.append(row);

      const ground = css.token("--ground-center");
      expect(noWidth(getComputedStyle(plain).borderTopWidth)).toBe(true);
      const box = getComputedStyle(excerpt);
      expect(box.borderTopWidth).toBe("1px");
      expect(box.borderTopColor).toBe(css.token("--danger"));
      expect(box.borderRadius).toBe("8px");
      expect(unpainted(box.backgroundColor)).toBe(true);
      for (const danger of [lines, more, failed]) {
        expect(getComputedStyle(danger).color).toBe(css.token("--danger"));
        expect(
          contrastRatio(getComputedStyle(danger).color, ground!),
          danger.className,
        ).toBeGreaterThanOrEqual(4.5);
      }

      row.remove();
    },
  );

  it("colours a diff's lines with the diff tokens and rules it on the left, nothing more", () => {
    const css = assembleCssProof(sheets, "light");
    injectToolRules(css);

    const diff = el("div", "workspace-chat-tool-output is-diff");
    const lines = el("div", "workspace-chat-tool-output-lines");
    const added = el("div", "workspace-chat-tool-output-line is-added", "+ a");
    const removed = el("div", "workspace-chat-tool-output-line is-removed", "- b");
    const hunk = el("div", "workspace-chat-tool-output-line is-hunk", "@@ c");
    lines.append(added, removed, hunk);
    diff.append(lines);
    document.body.append(diff);

    // The diff text tokens are colour mixes of the tone with the ink, which the
    // proof cannot compute, so each rule is read for the tone it mixes.
    const mixOf = (selector: string, tone: string) =>
      expect(css.rulesFor(selector)).toContain(`color-mix(in srgb, ${css.token(tone)}`);
    mixOf(".workspace-chat-tool-output-line.is-added", "--tone-add");
    mixOf(".workspace-chat-tool-output-line.is-removed", "--tone-del");
    mixOf(".workspace-chat-tool-output-line.is-hunk", "--tone-warn");
    // The only edge a diff draws is a rule on its inline start.
    expect(
      css.rulesFor(".workspace-chat-tool-output.is-diff .workspace-chat-tool-output-lines"),
    ).toContain("border-inline-start: 2px solid");
    expect(noWidth(getComputedStyle(diff).borderTopWidth)).toBe(true);

    diff.remove();
  });

  it.each(["light", "dark"] as const)(
    "reads the end of a line in order, with the fact at the far edge, in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      injectToolRules(css);

      const row = el("div", "workspace-chat-entry workspace-chat-tool is-running");
      const summary = el("div", "workspace-chat-tool-summary");
      const mark = el("span", "workspace-command-dot");
      const failed = el("span", "workspace-chat-tool-failed", "failed");
      const running = el("span", "workspace-chat-tool-running");
      const done = el("span", "workspace-chat-tool-done", "✓");
      const exit = el("span", "workspace-command-exit", "exit 0");
      summary.append(exit, done, running, failed, mark);
      row.append(summary);
      document.body.append(row);

      const order = [mark, failed, running, done, exit].map((node) => getComputedStyle(node).order);
      expect(order).toEqual(["1", "2", "3", "4", "5"]);
      expect(getComputedStyle(exit).marginInlineStart).toBe("auto");
      const dot = getComputedStyle(running);
      expect(dot.width).toBe("6px");
      expect(dot.height).toBe("6px");
      expect(dot.borderRadius).toBe("999px");
      expect(dot.backgroundColor).toBe(css.token("--tone-live"));

      row.remove();
    },
  );

  it("keeps the disclosure mark in a strip of the line's own, hidden until the line is reached", () => {
    const css = assembleCssProof(sheets, "light");
    injectToolRules(css);

    const row = el("div", "workspace-chat-entry workspace-chat-tool");
    const details = document.createElement("details");
    const summary = document.createElement("summary");
    summary.className = "workspace-chat-tool-summary";
    details.append(summary);
    row.append(details);
    document.body.append(row);

    expect(css.rulesFor(".workspace-chat-tool summary::after")).toContain('content: "▾"');
    // Pseudo-element styles are not computed here, so the rules are read.
    const mark = css.rulesFor(".workspace-chat-tool summary::after");
    expect(mark).toContain("position: absolute");
    expect(mark).toContain("opacity: 0");
    expect(css.rulesFor(".workspace-chat-tool summary:hover::after")).toContain("opacity: 1");
    // The strip is the line's own padding, so no row ends sooner than another.
    expect(getComputedStyle(summary).paddingRight).toBe("16px");

    row.remove();
  });

  it("flattens a group: a line with its rows indented under it, no fill and no gap", () => {
    const css = assembleCssProof(sheets, "light");
    injectToolRules(css);

    const group = el(
      "details",
      "workspace-chat-entry workspace-chat-tool workspace-chat-tool-group",
    );
    const summary = el("summary", "workspace-chat-tool-group-summary");
    const count = el("span", "workspace-chat-tool-group-count", "3 tool calls");
    const body = el("div", "workspace-chat-tool-group-body");
    summary.append(count);
    group.append(summary, body);
    document.body.append(group);

    expect(unpainted(getComputedStyle(group).backgroundColor)).toBe(true);
    expect(unpainted(getComputedStyle(summary).backgroundColor)).toBe(true);
    expect(getComputedStyle(count).color).toBe(css.token("--ink-soft"));
    const bodyStyle = getComputedStyle(body);
    expect(bodyStyle.marginInlineStart).toBe("12px");
    expect(bodyStyle.flexDirection).toBe("column");
    expect(bodyStyle.gap).not.toBe("4px");

    group.remove();
  });
});
