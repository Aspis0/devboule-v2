// @vitest-environment happy-dom

// The Projects page's own rules against the real stylesheets. Bare
// single-class selectors in the light theme only (cssProof's scope); the
// dark theme and anything it cannot see belong to a live check.

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
  read("src/features/settings/diagnostics.css"),
  read("src/features/settings/devices.css"),
  read("src/features/oracle/oracle.css"),
  read("src/features/settings/providers.css"),
  read("src/features/settings/profiles.css"),
  read("src/features/settings/projects.css"),
  read("src/features/settings/settings.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("projects page (real stylesheets, no app launch)", () => {
  it("draws the Add action as a bare glyph, never a dashed box", () => {
    proof.inject([".settings-add"]);
    const add = box("settings-add");
    const style = getComputedStyle(add);
    expect(style.borderTopStyle).toBe("none");
    expect(style.width).toBe("24px");
    expect(proof.rulesFor(".settings-add")).not.toMatch(/dashed/);
  });

  it("keeps workspace paths on the shell's mono meta face", () => {
    // The face is the shell's `.settings-card-meta`; the Projects page adds no
    // rule of its own for it, so the path lines cannot drift from the shell.
    proof.inject([".settings-card-meta"]);
    const meta = box("settings-card-meta");
    expect(getComputedStyle(meta).fontFamily).toMatch(/monospace|JetBrains/i);
  });
});
