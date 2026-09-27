// @vitest-environment happy-dom

// The Providers row geometry against the real stylesheets in bundle order.
// Scope, stated plainly: cssProof models bare single-class selectors in the
// light theme only (see its header) — this suite proves the rules exist with
// the spec's values, not the rendered cascade. Anything it cannot see
// (flex line-breaking, the dark theme, descendant conflicts) belongs to a
// live check and is listed in the slice report.
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

/** Every selector in the sheet whose rule sets a monospace family. */
function monoSelectors(css: string): string[] {
  const found: string[] = [];
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const block of stripped.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selector = block[1] ?? "";
    const body = block[2] ?? "";
    if (/JetBrains Mono|monospace/i.test(body)) {
      for (const part of selector.split(",")) found.push(part.trim().replace(/\s+/g, " "));
    }
  }
  return found;
}

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("providers row geometry (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle: tokens, global (main.tsx, static), then
  // providers.css BEFORE settings.css — SettingsSurface.tsx imports the
  // ProvidersPanel (line 11) ahead of "./settings.css" (line 23), and module
  // evaluation follows declaration order.
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/providers.css"),
    read("src/features/settings/settings.css"),
  ]);

  it("holds installed rows at h44", () => {
    proof.inject([".prov-row"]);
    expect(getComputedStyle(box("prov-row")).height).toBe("44px");
  });

  it("sets provider names at 14px", () => {
    proof.inject([".prov-name"]);
    expect(getComputedStyle(box("prov-name")).fontSize).toBe("14px");
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

  it("holds the row card to the same width as the content column", () => {
    proof.inject([".prov-card", ".settings-main-inner"]);
    const card = getComputedStyle(box("prov-card")).maxWidth;
    const column = getComputedStyle(box("settings-main-inner")).maxWidth;
    expect(card).toBe("720px");
    expect(card).toBe(column);
  });

  it("gives the glyph its own 14px box, not the workspace sheet's", () => {
    proof.inject([".prov-glyph"]);
    const glyph = box("prov-glyph");
    expect(getComputedStyle(glyph).width).toBe("14px");
    expect(getComputedStyle(glyph).height).toBe("14px");
  });

  it("wraps the available row's consent and error blocks onto their own line", () => {
    // The install-path P1: without this rule the consent card shares the
    // flex line with the Install button and the name column crushes to zero.
    expect(proof.rulesFor(".prov-available-row > .provider-card-block")).toContain(
      "flex-basis: 100%",
    );
  });

  it("declares no page-level section label: the shell owns .settings-subheading", () => {
    expect(read("src/features/settings/providers.css")).not.toContain(".prov-section-label");
  });

  it("keeps mono type to code, never UI words", () => {
    const css = read("src/features/settings/providers.css");
    const allowed = new Set([
      ".prov-detail-code",
      ".provider-consent-command",
      ".provider-version",
      ".provider-update-error pre",
    ]);
    const seen = monoSelectors(css);
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.filter((selector) => !allowed.has(selector))).toEqual([]);
  });
});
