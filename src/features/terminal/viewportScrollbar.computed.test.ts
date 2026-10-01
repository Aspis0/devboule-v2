// @vitest-environment happy-dom
// The terminal viewport's scrollbar, read from the real sheets in bundle
// order: xterm.css loads after Workspace.css (its module is what pulls it in),
// so the scoped three-class rule is the one the cascade picks — for
// `overflow-y`, and for the scrollbar colours each theme resolves. What these
// prove is the cascade and the declarations; the painted bar is checked live.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof, type CssTheme } from "../workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");
const read = (rel: string) => readFileSync(resolve(rootDir, rel), "utf8");

// Bundle order: the app's own sheet first, the vendor sheet last — the order
// in which the terminal's dynamic import appends it.
const SHEETS = [
  read("src/styles/tokens.css"),
  read("src/features/workspace/Workspace.css"),
  read("node_modules/@xterm/xterm/css/xterm.css"),
];

const SCOPED = ".workspace-terminal-host .xterm .xterm-viewport";
const VENDOR = ".xterm .xterm-viewport";

afterEach(() => {
  removeCssProof();
  document.body.replaceChildren();
});

function viewportEl(): HTMLElement {
  const host = document.createElement("div");
  host.className = "workspace-terminal-host";
  const xterm = document.createElement("div");
  xterm.className = "xterm";
  const viewport = document.createElement("div");
  viewport.className = "xterm-viewport";
  xterm.appendChild(viewport);
  host.appendChild(xterm);
  document.body.appendChild(host);
  return viewport;
}

function scrollbarColor(theme: CssTheme): { thumb: string; track: string } {
  const body = assembleCssProof(SHEETS, theme).rulesFor(SCOPED);
  const found = /scrollbar-color:\s*([^;]+)/.exec(body)?.[1]?.trim() ?? "";
  const [thumb = "", track = ""] = found.split(/\s+/);
  return { thumb, track };
}

describe("the viewport scrollbar, computed from the real sheets", () => {
  it("selects overflow-y: auto over xterm.css's forced scroll in the cascade", () => {
    assembleCssProof(SHEETS, "light").inject([SCOPED, VENDOR]);
    expect(getComputedStyle(viewportEl()).overflowY).toBe("auto");
  });

  it("resolves scrollbar-color to distinct thumb and track tokens in both themes", () => {
    const light = scrollbarColor("light");
    const dark = scrollbarColor("dark");
    for (const [theme, colors] of [
      ["light", light],
      ["dark", dark],
    ] as const) {
      expect(colors.thumb, theme).toMatch(/^#/);
      expect(colors.track, theme).toMatch(/^#/);
      expect(colors.thumb, theme).not.toBe(colors.track);
    }
    // The tokens are per-theme, so the bar cannot carry one theme's colour
    // into the other.
    expect(dark.thumb).not.toBe(light.thumb);
    expect(dark.track).not.toBe(light.track);
  });
});
