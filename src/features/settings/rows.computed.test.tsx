// @vitest-environment happy-dom

// The shared row pattern against the live stylesheets: rows.css and the shell
// sheet, the only ones a Settings page loads. Bare single-class selectors in
// the light theme (cssProof's scope).

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const proof = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/settings/settings.css"),
  read("src/features/settings/rows.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("shared row pattern (live stylesheets)", () => {
  it("draws over-limit figures in the danger tone on a row control", () => {
    proof.inject([".settings-value-danger"]);
    const control = document.createElement("div");
    control.className = "settings-row-control";
    const figure = document.createElement("span");
    figure.className = "settings-value-danger";
    figure.textContent = "2";
    control.appendChild(figure);
    document.body.appendChild(control);
    // --danger-deep aliases --danger; token() reads one level, so name the base.
    expect(getComputedStyle(figure).color).toBe(proof.token("--danger"));
  });
  it("draws a failed save as an error line in the danger colour, not a caption", () => {
    proof.inject([".settings-error"]);
    const line = document.createElement("p");
    line.className = "settings-error";
    line.setAttribute("role", "alert");
    line.textContent = "The close choice could not be saved.";
    document.body.appendChild(line);
    expect(getComputedStyle(line).color).toBe(proof.token("--danger"));
  });
});
