// @vitest-environment happy-dom

// The Devices card language against the real stylesheets in bundle order.
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

describe("devices card geometry (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle: tokens, global (main.tsx, static), then
  // devices.css BEFORE settings.css — SettingsSurface.tsx imports the
  // DevicesPanel (line 5) ahead of "./settings.css" (line 23), and module
  // evaluation follows declaration order.
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/devices.css"),
    read("src/features/settings/settings.css"),
  ]);

  it("holds page cards at max-width 720, r12, on the card ground", () => {
    proof.inject([".dev-card", ".settings-main-inner"]);
    const card = box("dev-card");
    const style = getComputedStyle(card);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    expect(proof.rulesFor(".dev-card")).toContain(proof.token("--panel-card"));
    const column = getComputedStyle(box("settings-main-inner")).maxWidth;
    expect(style.maxWidth).toBe(column);
  });

  it("divides stacked rows with the line token", () => {
    expect(proof.rulesFor(".dev-row-wrap + .dev-row-wrap")).toContain(proof.token("--line"));
  });

  it("sets row names at 14px", () => {
    proof.inject([".dev-name"]);
    expect(getComputedStyle(box("dev-name")).fontSize).toBe("14px");
  });

  it("sets row meta and status at 12px", () => {
    proof.inject([".dev-meta", ".dev-status"]);
    expect(getComputedStyle(box("dev-meta")).fontSize).toBe("12px");
    expect(getComputedStyle(box("dev-status")).fontSize).toBe("12px");
  });

  it("keeps the status line at 12 with the live dot on the live tone", () => {
    proof.inject([".dev-status", ".dev-dot-live"]);
    expect(getComputedStyle(box("dev-status")).fontSize).toBe("12px");
    expect(proof.rulesFor(".dev-dot-live")).toContain(proof.token("--tone-live"));
  });

  it("gives the glyph its own 14px box", () => {
    proof.inject([".dev-glyph"]);
    const glyph = box("dev-glyph");
    expect(getComputedStyle(glyph).width).toBe("14px");
    expect(getComputedStyle(glyph).height).toBe("14px");
  });

  it("sizes the row kebab at 26px like the house icon buttons", () => {
    proof.inject([".dev-kebab"]);
    const kebab = box("dev-kebab");
    expect(getComputedStyle(kebab).width).toBe("26px");
    expect(getComputedStyle(kebab).height).toBe("26px");
  });

  it("declares no page-level section label: the shell owns .settings-subheading", () => {
    expect(read("src/features/settings/devices.css")).not.toContain(".dev-section-label");
    expect(read("src/features/settings/devices.css")).not.toContain(".settings-subheading");
  });

  it("keeps mono type to code, never UI words", () => {
    // The pairing code is typed off a screen, the fingerprint is read aloud
    // in fours, and the two pairing inputs are typed verbatim: every kept
    // mono face is characters the person reads one by one. (`.device-field`
    // keeps its name: the rule is shared with the Agents panel's forms.)
    // The role chip and every meta line are UI words and stay sans.
    const css = read("src/features/settings/devices.css");
    const allowed = new Set([".dev-pair-code", ".dev-fingerprint", ".device-field input"]);
    const seen = monoSelectors(css);
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.filter((selector) => !allowed.has(selector))).toEqual([]);
  });

  it("keeps the shared confirm rules the Profiles page reuses", () => {
    // `ProfileRow` (R17-2) renders `device-actions`, `device-inline-confirm`
    // and `device-copy` without importing this sheet: the settings chunk
    // carries every page's CSS, so these rules live here and must not move.
    const css = read("src/features/settings/devices.css");
    for (const selector of [
      ".device-actions",
      ".device-inline-confirm",
      ".device-copy",
      ".device-field",
      ".device-error",
    ]) {
      expect(css, selector).toContain(`${selector} {`);
    }
  });
});
