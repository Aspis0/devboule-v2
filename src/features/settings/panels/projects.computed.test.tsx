// @vitest-environment happy-dom

// The Projects page card language against the real stylesheets in bundle
// order: tokens, global, the shell sheet, then projects.css. Bare
// single-class selectors in the light theme only (cssProof's scope); the
// dark theme and anything it cannot see belong to a live check and are
// listed in the slice report.

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

const proof = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/settings/settings.css"),
  read("src/features/settings/projects.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("projects cards (real stylesheets, no app launch)", () => {
  it("grounds the project list on the house card", () => {
    proof.inject([".proj-card"]);
    const card = box("proj-card");
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

  it("sets project names at label 14, sans", () => {
    proof.inject([".proj-name"]);
    const name = document.createElement("span");
    name.className = "proj-name";
    name.textContent = "real-project";
    document.body.appendChild(name);
    const style = getComputedStyle(name);
    expect(style.fontSize).toBe("14px");
    expect(style.fontFamily).not.toMatch(/monospace|JetBrains/i);
  });

  it("keeps workspace paths readable at meta 12 without losing their mono face", () => {
    // The paths are data, so the shell's mono meta face stays; the page
    // only lifts the size to the spec's 12.
    const rules = proof.rulesFor("#settings-panel-projects .settings-card-meta");
    expect(rules).toContain("font-size: 12px");
    expect(rules).not.toMatch(/monospace|JetBrains/i);
  });

  it("keeps the project sections on the shell's vertical rhythm", () => {
    expect(proof.rulesFor("#settings-panel-projects > section")).toContain("margin-bottom: 18px");
  });
});
