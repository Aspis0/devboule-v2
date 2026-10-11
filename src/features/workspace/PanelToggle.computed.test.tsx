// @vitest-environment happy-dom

// The side-panel toggle against the live sheets: a 28px header button, and a
// key tip that is hidden at rest. Hover and focus reveal it in the sheet, which
// cannot be driven here, so the reveal rules are read from the cascade's text.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "./cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const proof = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/workspace/PanelToggle.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("the side-panel toggle (live stylesheets)", () => {
  it("draws the header button 28px square", () => {
    proof.inject([".panel-toggle"]);
    const toggle = document.createElement("button");
    toggle.className = "panel-toggle";
    document.body.appendChild(toggle);

    expect(getComputedStyle(toggle).width).toBe("28px");
    expect(getComputedStyle(toggle).height).toBe("28px");
  });

  it("keeps the key tip out of sight at rest", () => {
    proof.inject([".panel-toggle", ".panel-key-tip"]);
    const toggle = document.createElement("button");
    toggle.className = "panel-toggle";
    const tip = document.createElement("span");
    tip.className = "panel-key-tip";
    toggle.appendChild(tip);
    document.body.appendChild(toggle);

    expect(getComputedStyle(tip).display).toBe("none");
  });

  it("reveals the key tip on hover and on focus, with no delay", () => {
    expect(proof.rulesFor(".panel-toggle:hover > .panel-key-tip")).toContain("display: flex");
    expect(proof.rulesFor(".panel-toggle:focus-visible > .panel-key-tip")).toContain(
      "display: flex",
    );
  });
});
