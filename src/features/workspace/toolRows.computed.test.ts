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

function opacityOf(color: string): number {
  const parts = color.match(/[\d.]+/g);
  return parts !== null && parts.length === 4 ? Number(parts[3]) : 1;
}

function contrastRatio(foreground: string, background: string): number {
  // A fully transparent foreground shows whatever is beneath it: no contrast.
  if (opacityOf(foreground) === 0 || opacityOf(background) === 0) return 1;
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

describe("tool row computed styles", () => {
  it.each(["light", "dark"] as const)(
    "keeps open tool text and icons readable in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      const lightCss = assembleCssProof(sheets, "light");
      // The dark proof must actually resolve the dark token block; this fails
      // if the resolver is bypassed because --code-bg differs by theme.
      if (theme === "dark") {
        expect(css.token("--code-bg")).not.toBe(lightCss.token("--code-bg"));
      }
      css.inject([
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open]",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] > summary",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] .workspace-chat-tool-summary-text",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] .workspace-chat-tool-label",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] .workspace-chat-tool-failed",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] .workspace-chat-tool-location",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] .workspace-chat-tool-interrupted",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] summary::after",
        ".workspace-chat-tool:not(.workspace-chat-tool-group)[open] summary > svg",
        ".workspace-chat-tool-label",
        ".workspace-chat-tool-failed",
        ".workspace-chat-tool-location",
        ".workspace-chat-tool-interrupted",
        ".workspace-chat-tool.is-running .workspace-chat-tool-running",
        ".workspace-chat-tool-running",
        ".workspace-chat-tool summary > svg",
        ".workspace-chat-tool summary::after",
        ".workspace-chat-tool-body",
        ".workspace-chat-tool-group",
        ".workspace-chat-tool[open]",
        ".workspace-chat-tool[open] > summary",
        ".workspace-chat-tool[open] .workspace-chat-tool-failed",
        ".workspace-chat-tool[open] .workspace-chat-tool-interrupted",
        ".workspace-chat-tool[open] summary::after",
        ".workspace-chat-tool-group[open]",
        ".workspace-chat-tool-group[open] > summary",
        ".workspace-chat-tool-group-body",
        ".workspace-chat-tool-group summary::after",
        ".workspace-chat-tool-group-summary-text",
        ".workspace-chat-tool-group-count",
        ".workspace-chat-tool-group.is-interrupted .workspace-chat-tool-group-summary-text",
        ".workspace-chat-tool-group .workspace-chat-tool-interrupted",
        ".workspace-chat-tool-group .workspace-chat-tool-failed",
        ".workspace-chat-tool-interrupted",
        ".workspace-chat-tool-failed",
        ".workspace-chat-tool-group summary > svg",
        ".workspace-chat-tool-group.is-running .workspace-chat-tool-running",
        ".workspace-chat-tool-interrupted",
        ".workspace-chat-tool-location",
      ]);

      const tool = document.createElement("details");
      tool.className = "workspace-chat-entry workspace-chat-tool is-failed";
      tool.open = true;
      const summary = document.createElement("summary");
      const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      const label = document.createElement("span");
      label.className = "workspace-chat-tool-label";
      label.textContent = "Shell";
      const command = document.createElement("span");
      command.className = "workspace-chat-tool-summary-text";
      command.textContent = "pnpm test";
      const failed = document.createElement("span");
      failed.className = "workspace-chat-tool-failed";
      failed.textContent = "×";
      summary.append(icon, label, command, failed);
      tool.append(summary);

      const body = document.createElement("div");
      body.className = "workspace-chat-tool-body";
      const location = document.createElement("span");
      location.className = "workspace-chat-tool-location";
      location.textContent = "src/main.ts";
      body.append(location);
      tool.append(body);
      document.body.append(tool);

      const interruptedTool = document.createElement("details");
      interruptedTool.className = "workspace-chat-entry workspace-chat-tool is-interrupted";
      interruptedTool.open = true;
      const interruptedSummary = document.createElement("summary");
      const interruptedLabel = document.createElement("span");
      interruptedLabel.className = "workspace-chat-tool-label";
      interruptedLabel.textContent = "Shell";
      const interrupted = document.createElement("span");
      interrupted.className = "workspace-chat-tool-interrupted";
      interrupted.textContent = "Interrupted";
      interruptedSummary.append(interruptedLabel, interrupted);
      interruptedTool.append(interruptedSummary);
      document.body.append(interruptedTool);

      const codeBg = css.token("--code-bg");
      expect(codeBg).toBeDefined();
      for (const text of [label, command, failed, location, interruptedLabel, interrupted]) {
        const color = getComputedStyle(text).color;
        expect(contrastRatio(color, codeBg!)).toBeGreaterThanOrEqual(4.5);
      }
      expect(getComputedStyle(interrupted).backgroundColor).toBe(codeBg);
      expect(contrastRatio(getComputedStyle(icon).color, codeBg!)).toBeGreaterThanOrEqual(4.5);
      expect(
        contrastRatio(getComputedStyle(summary, "::after").color, codeBg!),
      ).toBeGreaterThanOrEqual(4.5);
      expect(css.rulesFor(".workspace-chat-tool summary::after")).toContain('content: "▾"');
      expect(getComputedStyle(label).fontFamily).not.toContain("JetBrains Mono");
      expect(getComputedStyle(interruptedLabel).fontFamily).not.toContain("JetBrains Mono");
      expect(getComputedStyle(command).fontFamily).toContain("JetBrains Mono");
      expect(getComputedStyle(body).fontFamily).toContain("JetBrains Mono");
      expect(
        contrastRatio(getComputedStyle(location).borderTopColor, codeBg!),
      ).toBeGreaterThanOrEqual(3);
      tool.remove();
      interruptedTool.remove();
    },
  );

  it.each(["light", "dark"] as const)(
    "keeps every expanded group summary span readable on the tool fill in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([
        ".workspace-chat-tool",
        ".workspace-chat-tool[open]",
        ".workspace-chat-tool[open] > summary",
        ".workspace-chat-tool[open] .workspace-chat-tool-failed",
        ".workspace-chat-tool[open] .workspace-chat-tool-interrupted",
        ".workspace-chat-tool[open] summary::after",
        ".workspace-chat-tool-group",
        ".workspace-chat-tool-group[open]",
        ".workspace-chat-tool-group[open] > summary",
        ".workspace-chat-tool-group-body",
        ".workspace-chat-tool-group summary::after",
        ".workspace-chat-tool-group-count",
        ".workspace-chat-tool-group-summary-text",
        ".workspace-chat-tool-group.is-interrupted .workspace-chat-tool-group-summary-text",
        ".workspace-chat-tool-group .workspace-chat-tool-failed",
        ".workspace-chat-tool-group .workspace-chat-tool-interrupted",
        ".workspace-chat-tool-failed",
        ".workspace-chat-tool-interrupted",
        ".workspace-chat-tool.is-failed .workspace-chat-tool-failed",
        ".workspace-chat-tool-group summary > svg",
        ".workspace-chat-tool-running",
        ".workspace-chat-tool-group.is-running .workspace-chat-tool-running",
      ]);
      const group = document.createElement("details");
      group.className =
        "workspace-chat-entry workspace-chat-tool workspace-chat-tool-group is-interrupted is-failed";
      group.open = true;
      const summary = document.createElement("summary");
      summary.className = "workspace-chat-tool-group-summary";
      const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      const count = document.createElement("span");
      count.className = "workspace-chat-tool-group-count";
      count.textContent = "3 tool calls";
      const text = document.createElement("span");
      text.className = "workspace-chat-tool-group-summary-text";
      text.textContent = "Edited 1 file";
      const failed = document.createElement("span");
      failed.className = "workspace-chat-tool-failed";
      failed.textContent = "×";
      const interrupted = document.createElement("span");
      interrupted.className = "workspace-chat-tool-interrupted";
      interrupted.textContent = "Interrupted";
      summary.append(icon, count, text, failed, interrupted);
      const body = document.createElement("div");
      body.className = "workspace-chat-tool-group-body";
      const row = document.createElement("details");
      row.className = "workspace-chat-entry workspace-chat-tool";
      group.append(summary, body);
      body.append(row);
      document.body.append(group);

      const fillTool = css.token("--fill-tool");
      expect(getComputedStyle(group).backgroundColor).toBe("transparent");
      expect(getComputedStyle(summary).backgroundColor).toBe(fillTool);
      expect(getComputedStyle(summary).borderRadius).toBe("6px");
      for (const span of [count, text, failed, interrupted]) {
        const visibleBackground =
          span === interrupted ? getComputedStyle(span).backgroundColor : fillTool!;
        expect(
          contrastRatio(getComputedStyle(span).color, visibleBackground),
        ).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrastRatio(getComputedStyle(icon).color, fillTool!)).toBeGreaterThanOrEqual(4.5);
      expect(
        contrastRatio(getComputedStyle(summary, "::after").color, fillTool!),
      ).toBeGreaterThanOrEqual(4.5);
      expect(getComputedStyle(body).gap).toBe("2px");
      expect(getComputedStyle(row).backgroundColor).toBe(fillTool);
      expect(getComputedStyle(row).borderRadius).toBe("6px");
      group.remove();
    },
  );

  it.each([
    {
      theme: "light" as CssTheme,
      state: "running",
      classes: "workspace-chat-entry workspace-chat-tool is-running",
    },
    {
      theme: "dark" as CssTheme,
      state: "running",
      classes: "workspace-chat-entry workspace-chat-tool is-running",
    },
    {
      theme: "light" as CssTheme,
      state: "failed",
      classes: "workspace-chat-entry workspace-chat-tool is-failed",
    },
    {
      theme: "dark" as CssTheme,
      state: "failed",
      classes: "workspace-chat-entry workspace-chat-tool is-failed",
    },
  ])("styles a single $state tool row in the $theme theme", ({ theme, state, classes }) => {
    const css = assembleCssProof(sheets, theme);
    css.inject([
      ".workspace-chat-tool",
      ".workspace-chat-tool-running",
      ".workspace-chat-tool.is-running .workspace-chat-tool-running",
      ".workspace-chat-tool-failed",
    ]);
    const row = document.createElement("details");
    row.className = classes;
    const summary = document.createElement("summary");
    const marker = document.createElement("span");
    if (state === "running") {
      marker.className = "workspace-chat-tool-running";
      marker.setAttribute("role", "img");
      marker.setAttribute("aria-label", "Running");
    } else {
      marker.className = "workspace-chat-tool-failed";
      marker.textContent = "×";
      marker.setAttribute("role", "img");
      marker.setAttribute("aria-label", "Failed");
    }
    summary.append(marker);
    row.append(summary);
    document.body.append(row);

    if (state === "running") {
      // The mockup's `.tool-status { margin-left: auto }`: the dot owns the
      // trailing space and the chevron follows it.
      expect(getComputedStyle(marker).marginLeft).toBe("auto");
      expect(getComputedStyle(marker).width).toBe("6px");
      expect(
        contrastRatio(getComputedStyle(marker).backgroundColor, css.token("--fill-tool")!),
      ).toBeGreaterThanOrEqual(3);
    } else {
      expect(getComputedStyle(marker).color).toBe(css.token("--danger"));
      expect(
        contrastRatio(getComputedStyle(marker).color, css.token("--fill-tool")!),
      ).toBeGreaterThanOrEqual(4.5);
    }
    row.remove();
  });

  it("keeps a non-running row's chevron on the trailing edge", () => {
    const css = assembleCssProof(sheets);
    css.inject([
      ".workspace-chat-tool summary::after",
      ".workspace-chat-tool:not(.is-running) summary::after",
    ]);
    const row = document.createElement("details");
    row.className = "workspace-chat-entry workspace-chat-tool";
    const summary = document.createElement("summary");
    row.append(summary);
    document.body.append(row);

    // Happy DOM resolves no computed margin on a generated pseudo-element
    // (probed: `getComputedStyle(summary, "::after").marginLeft` is `""`),
    // so the declaration is asserted through the rule source.
    expect(css.rulesFor(".workspace-chat-tool:not(.is-running) summary::after")).toContain(
      "margin-left: auto",
    );
    row.remove();
  });

  it("lets a row's summary span the full row so the chevron owns the trailing edge", () => {
    const css = assembleCssProof(sheets);
    css.inject([".workspace-chat-tool", ".workspace-chat-tool summary"]);
    const row = document.createElement("details");
    row.className = "workspace-chat-entry workspace-chat-tool";
    const summary = document.createElement("summary");
    row.append(summary);
    document.body.append(row);

    // A content-width summary leaves the chevron's auto margin no free space,
    // so the chevron sits after the label instead of the trailing edge (live
    // finding: closed inner rows of an open group).
    expect(getComputedStyle(summary).flexGrow).toBe("1");
    row.remove();
  });

  it("keeps the label whole by routing the row's overflow through its text block", () => {
    assembleCssProof(sheets).inject([
      ".workspace-chat-tool-text",
      ".workspace-chat-tool-text.has-summary .workspace-chat-tool-label",
      ".workspace-chat-tool-label",
      ".workspace-chat-tool-summary-text",
    ]);
    const block = document.createElement("span");
    block.className = "workspace-chat-tool-text has-summary";
    const label = document.createElement("span");
    label.className = "workspace-chat-tool-label";
    const summaryText = document.createElement("span");
    summaryText.className = "workspace-chat-tool-summary-text";
    block.append(label, summaryText);
    document.body.append(block);

    // The block shrinks at row level, so the row never outgrows the
    // transcript; inside it only the summary yields, never the label.
    expect(getComputedStyle(label).flexShrink).toBe("0");
    // The floor, when a summary shares the block: the cap leaves the gap plus
    // a 40 px strip, so the yielding text keeps ~6 characters.
    expect(getComputedStyle(label).maxWidth).toBe("calc(100% - 48px)");
    expect(getComputedStyle(label).textOverflow).toBe("ellipsis");
    expect(getComputedStyle(label).whiteSpace).toBe("nowrap");
    expect(getComputedStyle(block).flexShrink).toBe("10");
    expect(getComputedStyle(block).flexBasis).toBe("100%");
    expect(getComputedStyle(block).gap).toBe("8px");
    expect(getComputedStyle(block).minWidth).toBe("0");
    expect(getComputedStyle(block).overflow).toBe("hidden");
    expect(getComputedStyle(summaryText).textOverflow).toBe("ellipsis");
    expect(getComputedStyle(summaryText).minWidth).toBe("0");
    expect(Number.parseFloat(getComputedStyle(summaryText).flexShrink)).toBeGreaterThan(0);
    block.remove();

    // Without a claimant the label keeps the block's full width: the strip
    // would otherwise ellipsize a long bare name ~8 characters early.
    const loneBlock = document.createElement("span");
    loneBlock.className = "workspace-chat-tool-text";
    const loneLabel = document.createElement("span");
    loneLabel.className = "workspace-chat-tool-label";
    loneBlock.append(loneLabel);
    document.body.append(loneBlock);
    expect(getComputedStyle(loneLabel).maxWidth).toBe("100%");
    loneBlock.remove();
  });

  it.each(["light", "dark"] as const)(
    "keeps a running group's dot at the trailing edge in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([".workspace-chat-tool-running", ".workspace-chat-tool-group"]);
      const group = document.createElement("details");
      group.className =
        "workspace-chat-entry workspace-chat-tool workspace-chat-tool-group is-running";
      const summary = document.createElement("summary");
      const dot = document.createElement("span");
      dot.className = "workspace-chat-tool-running";
      summary.append(dot);
      group.append(summary);
      document.body.append(group);

      expect(getComputedStyle(dot).marginLeft).toBe("auto");
      expect(
        contrastRatio(getComputedStyle(dot).backgroundColor, css.token("--fill-tool")!),
      ).toBeGreaterThanOrEqual(3);
      group.remove();
    },
  );

  it.each(["light", "dark"] as const)(
    "keeps expanded groups transparent with separate tool rows and 2 px gaps in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      css.inject([
        ".workspace-chat-tool-group",
        ".workspace-chat-tool-group[open]",
        ".workspace-chat-tool-group[open] > summary",
        ".workspace-chat-tool-group-summary",
        ".workspace-chat-tool-group-body",
        ".workspace-chat-tool",
        ".workspace-chat-tool[open]",
        ".workspace-chat-tool[open] > summary",
        ".workspace-chat-tool[open] summary::after",
      ]);
      const group = document.createElement("details");
      group.className = "workspace-chat-entry workspace-chat-tool workspace-chat-tool-group";
      group.open = true;
      const summary = document.createElement("summary");
      summary.className = "workspace-chat-tool-group-summary";
      const body = document.createElement("div");
      body.className = "workspace-chat-tool-group-body";
      const row = document.createElement("details");
      row.className = "workspace-chat-tool";
      group.append(summary, body);
      body.append(row);
      document.body.append(group);

      expect(getComputedStyle(group).backgroundColor).toBe("transparent");
      expect(getComputedStyle(group).gap).toBe("2px");
      expect(getComputedStyle(summary).backgroundColor).toBe(css.token("--fill-tool"));
      expect(getComputedStyle(body).gap).toBe("2px");
      expect(getComputedStyle(row).backgroundColor).toBe(css.token("--fill-tool"));
      expect(getComputedStyle(row).borderRadius).toBe("6px");
      group.remove();
    },
  );
});
