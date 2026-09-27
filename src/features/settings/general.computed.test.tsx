// @vitest-environment happy-dom

// The This-machine card language against the real stylesheets in bundle
// order: tokens, global (main.tsx, static), general.css, then settings.css
// — SettingsSurface.tsx imports the Appearance section (line 7, which pulls
// general.css) ahead of "./settings.css" (line 23), and module evaluation
// follows declaration order. Scope, stated plainly: cssProof models bare
// single-class selectors in the light theme only — this suite proves the
// rules exist with the spec's values, not the rendered cascade.

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

describe("this-machine card language (real stylesheets, no app launch)", () => {
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/general.css"),
    read("src/features/settings/settings.css"),
  ]);

  it("holds the cards at max-width 720 with the r12 panel face", () => {
    proof.inject([".machine-card"]);
    const card = box("machine-card");
    const style = getComputedStyle(card);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    expect(style.backgroundColor).not.toBe("rgba(0, 0, 0, 0)");
  });

  it("sets row titles at 14 with the description at 12 below", () => {
    proof.inject([".machine-row-title", ".machine-row-desc"]);
    expect(getComputedStyle(box("machine-row-title")).fontSize).toBe("14px");
    const desc = getComputedStyle(box("machine-row-desc"));
    expect(desc.fontSize).toBe("12px");
  });

  it("sizes the notification switch at 34x20 with accent on", () => {
    proof.inject([".machine-switch", ".machine-switch-on"]);
    const toggle = box("machine-switch machine-switch-on");
    const style = getComputedStyle(toggle);
    expect(style.width).toBe("34px");
    expect(style.height).toBe("20px");
  });

  it("rings keyboard focus on the switch and the segmented options", () => {
    const css = read("src/features/settings/general.css");
    for (const selector of [
      ".machine-switch:focus-visible",
      ".machine-segment-option input:focus-visible",
    ]) {
      const at = css.indexOf(selector);
      if (at === -1) throw new Error(`${selector} rule is missing`);
      const body = css.slice(css.indexOf("{", at), css.indexOf("}", at));
      expect(body).toMatch(/outline:\s*2px solid/);
    }
  });

  it("keeps every face on this page in the UI sans (no mono descriptions)", () => {
    // The Layout row used to read through the mono retention input and the
    // mono card meta: scan the whole sheet, @-blocks included, so a
    // responsive tweak cannot smuggle a mono face past this line.
    const css = read("src/features/settings/general.css").replace(/\/\*[\s\S]*?\*\//g, "");
    expect(css).not.toMatch(/JetBrains Mono|monospace/i);
  });
});
