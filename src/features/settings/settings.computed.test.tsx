// @vitest-environment happy-dom

// The Settings shell geometry against the real stylesheets: the assembled
// sheets in bundle order (tokens, global, settings — main.tsx first, the
// lazy settings chunk after), so a cascade inversion like the content
// column stacking under the menu fails here, not live.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

function box(className: string): HTMLElement {
  const el = document.createElement("div");
  el.className = className;
  document.body.appendChild(el);
  return el;
}

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("settings shell layout (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle: tokens, global (main.tsx, static),
  // settings (lazy chunk, last).
  const { inject } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/settings.css"),
  ]);

  it("lays the menu beside the content, not above it", () => {
    inject([".surface-card", ".settings-surface"]);
    const surface = box("surface-card settings-surface");
    expect(getComputedStyle(surface).flexDirection).toBe("row");
  });

  it("holds the content column's cards at max-width 720", () => {
    inject([".settings-main-inner"]);
    const inner = box("settings-main-inner");
    expect(getComputedStyle(inner).maxWidth).toBe("720px");
  });

  it("scrolls the content column, never the surface", () => {
    inject([".surface-card", ".settings-surface", ".settings-main", ".settings-menu"]);
    const surface = box("surface-card settings-surface");
    const main = box("settings-main");
    const menu = box("settings-menu");
    expect(getComputedStyle(surface).overflow).toBe("hidden");
    expect(getComputedStyle(main).overflowY).toBe("auto");
    // The menu column stays put; short windows scroll it internally.
    expect(getComputedStyle(menu).overflowY).toBe("auto");
  });

  it("styles scrollbars the workspace way, not the native way", () => {
    inject([".settings-main"]);
    const main = box("settings-main");
    const style = getComputedStyle(main);
    expect(style.scrollbarWidth).toBe("thin");
    expect(style.scrollbarColor).not.toBe("");
  });
});
