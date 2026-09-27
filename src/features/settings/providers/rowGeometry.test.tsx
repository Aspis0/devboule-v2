// @vitest-environment happy-dom

// The Providers row geometry against the real stylesheets: the assembled
// sheets in bundle order (tokens, global, settings, providers last as the
// lazy chunk), so a row that is not h44 or a switch that is not 34x20 fails
// here, not live. Written after the stylesheet (the exception to the
// red-first rule on this slice); values cross-checked against
// `mockups/skeleton-settings.css`, which wins on values.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../../workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");

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

describe("providers row geometry (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle: tokens, global (main.tsx, static),
  // settings (lazy chunk), providers (this slice's lazy chunk, last).
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/settings.css"),
    read("src/features/settings/providers.css"),
  ]);

  it("holds installed rows at h44", () => {
    proof.inject([".prov-row"]);
    expect(getComputedStyle(box("prov-row")).height).toBe("44px");
  });

  it("sets provider names at 14 sans", () => {
    proof.inject([".prov-name"]);
    const name = box("prov-name");
    expect(getComputedStyle(name).fontSize).toBe("14px");
    expect(proof.rulesFor(".prov-name")).not.toMatch(/mono/i);
  });

  it("sizes the Devboule-tools switch at 34x20 with accent on", () => {
    proof.inject([".prov-switch", ".prov-switch-on"]);
    const toggle = box("prov-switch prov-switch-on");
    const style = getComputedStyle(toggle);
    expect(style.width).toBe("34px");
    expect(style.height).toBe("20px");
    expect(proof.rulesFor(".prov-switch-on")).toContain(proof.token("--accent"));
  });

  it("keeps the status line at 12 with the live dot on the live tone", () => {
    proof.inject([".prov-status", ".prov-dot-live"]);
    expect(getComputedStyle(box("prov-status")).fontSize).toBe("12px");
    expect(proof.rulesFor(".prov-dot-live")).toContain(proof.token("--tone-live"));
  });

  it("holds the row card at max-width 720 like the content column", () => {
    proof.inject([".prov-card"]);
    expect(getComputedStyle(box("prov-card")).maxWidth).toBe("720px");
  });
});
