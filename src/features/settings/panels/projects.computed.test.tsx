// @vitest-environment happy-dom

// The Projects page card language against the real stylesheets in the REAL
// bundle order, measured with `vite build` on this tree (SettingsSurface
// chunk byte offsets: diagnostics 55, devices 3427, oracle 7656, general
// 25685, providers 26993, profiles 33013, projects 38259, settings.css
// LAST at 39720+). Assembling any other order measures a fictional cascade:
// settings.css beats equal-specificity page rules, so the competing
// `.settings-card-title` class stays in the DOM below and every type
// assertion below would pass without it. Bare single-class selectors in
// the light theme only (cssProof's scope); the dark theme and anything it
// cannot see belong to a live check and are listed in the slice report.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../../workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const proof = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/settings/diagnostics.css"),
  read("src/features/settings/devices.css"),
  read("src/features/oracle/oracle.css"),
  read("src/features/settings/general.css"),
  read("src/features/settings/providers.css"),
  read("src/features/settings/profiles.css"),
  read("src/features/settings/projects.css"),
  read("src/features/settings/settings.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("projects cards (real stylesheets, no app launch)", () => {
  it("grounds the project list on the house card", () => {
    proof.inject([".proj-card"]);
    const card = document.createElement("div");
    card.className = "proj-card";
    document.body.appendChild(card);
    const style = getComputedStyle(card);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    expect(proof.rulesFor(".proj-card")).toContain(proof.token("--panel-card"));
    expect(proof.rulesFor(".proj-card")).toContain(proof.token("--line"));
  });

  it("divides project rows on the house line", () => {
    expect(read("src/features/settings/projects.css")).toContain(".proj-row + .proj-row");
    expect(proof.rulesFor(".proj-row + .proj-row")).toContain(proof.token("--line"));
  });

  it("renders project names at label 14 against the shell's own title class", () => {
    // The row keeps `settings-card-title` (its `display: block`), which the
    // shell sheet sets to 13px AFTER this page's sheet. The page rule wins
    // by specificity, not by order: without the id scope this renders 13px.
    proof.inject([".settings-card-title", "#settings-panel-projects .proj-name"]);
    const panel = document.createElement("div");
    panel.id = "settings-panel-projects";
    const name = document.createElement("span");
    name.className = "settings-card-title proj-name";
    name.textContent = "real-project";
    panel.appendChild(name);
    document.body.appendChild(panel);
    const style = getComputedStyle(name);
    expect(style.fontSize).toBe("14px");
    expect(style.fontFamily).not.toMatch(/monospace|JetBrains/i);
  });

  it("keeps workspace paths on the shell's mono meta face at meta 12", () => {
    // The effective face comes from the shell sheet (last in the bundle);
    // the page sheet only lifts the size. Drop the family there and every
    // path turns sans with a size-only assertion still green.
    proof.inject([".settings-card-meta", "#settings-panel-projects .settings-card-meta"]);
    const panel = document.createElement("div");
    panel.id = "settings-panel-projects";
    const meta = document.createElement("span");
    meta.className = "settings-card-meta";
    meta.textContent = "D:\\real-project";
    panel.appendChild(meta);
    document.body.appendChild(panel);
    const style = getComputedStyle(meta);
    expect(style.fontSize).toBe("12px");
    expect(style.fontFamily).toMatch(/monospace|JetBrains/i);
  });

  it("holds loading, error, rows and the Add action in one spaced stack", () => {
    // The stack is the page's only top reference (18px under the intro)
    // and its inter-block gap (8px): loading/error/empty lines sit in it,
    // never flush against the card.
    proof.inject([".proj-stack"]);
    const stack = document.createElement("div");
    stack.className = "proj-stack";
    document.body.appendChild(stack);
    const style = getComputedStyle(stack);
    expect(style.display).toBe("flex");
    expect(style.marginTop).toBe("18px");
    expect(proof.rulesFor(".proj-stack")).toContain("gap: 8px");
  });
});
