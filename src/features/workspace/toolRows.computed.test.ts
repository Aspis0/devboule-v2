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

describe("tool row computed styles", () => {
  it.each(["light", "dark"] as const)(
    "keeps open tool text and icons readable in the %s theme",
    (theme: CssTheme) => {
      const css = assembleCssProof(sheets, theme);
      document.documentElement.dataset.theme = theme;
      css.inject([
        ".workspace-chat-tool[open]",
        ".workspace-chat-tool[open] > summary",
        ".workspace-chat-tool[open] .workspace-chat-tool-summary-text",
        ".workspace-chat-tool[open] .workspace-chat-tool-label",
        ".workspace-chat-tool[open] .workspace-chat-tool-failed",
        ".workspace-chat-tool[open] .workspace-chat-tool-location",
        ".workspace-chat-tool[open] .workspace-chat-tool-interrupted",
        ".workspace-chat-tool[open] summary::after",
        ".workspace-chat-tool-label",
        ".workspace-chat-tool-failed",
        ".workspace-chat-tool-location",
        ".workspace-chat-tool-interrupted",
        ".workspace-chat-tool summary > svg",
        ".workspace-chat-tool summary::after",
        ".workspace-chat-tool-body",
      ]);

      const tool = document.createElement("details");
      tool.className = "workspace-chat-tool";
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
      const interrupted = document.createElement("span");
      interrupted.className = "workspace-chat-tool-interrupted";
      interrupted.textContent = "Interrupted";
      summary.append(icon, label, command, failed, interrupted);
      tool.append(summary);

      const body = document.createElement("div");
      body.className = "workspace-chat-tool-body";
      const location = document.createElement("span");
      location.className = "workspace-chat-tool-location";
      location.textContent = "src/main.ts";
      body.append(location);
      tool.append(body);
      document.body.append(tool);

      const codeBg = css.token("--code-bg");
      expect(codeBg).toBeDefined();
      for (const text of [label, command, failed, location, interrupted]) {
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
      expect(getComputedStyle(command).fontFamily).toContain("JetBrains Mono");
      expect(getComputedStyle(body).fontFamily).toContain("JetBrains Mono");
      tool.remove();
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
      ]);
      const group = document.createElement("details");
      group.className = "workspace-chat-tool-group";
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

      expect(getComputedStyle(group).backgroundColor).not.toBe(css.token("--fill-tool"));
      expect(getComputedStyle(group).gap).toBe("2px");
      expect(getComputedStyle(summary).backgroundColor).toBe(css.token("--fill-tool"));
      expect(getComputedStyle(body).gap).toBe("2px");
      expect(getComputedStyle(body).marginTop).not.toBe("4px");
      expect(getComputedStyle(row).backgroundColor).toBe(css.token("--fill-tool"));
      expect(getComputedStyle(row).borderRadius).toBe("6px");
      group.remove();
    },
  );
});
