// @vitest-environment happy-dom

// ChoiceGroup against the live tokens and its own sheet: the option sizes and
// the no-wrap rule that the Settings close row needs, checked through the cascade.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../features/workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const proof = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/components/ChoiceGroup.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

function mountGroup(vertical: boolean): HTMLElement {
  const group = document.createElement("div");
  group.className = vertical ? "choice-group choice-group-vertical" : "choice-group";
  group.setAttribute("role", "radiogroup");
  for (const label of ["Ask every time", "Keep running in the tray", "Quit Devboule"]) {
    const option = document.createElement("button");
    option.type = "button";
    option.setAttribute("role", "radio");
    option.setAttribute("aria-checked", label === "Ask every time" ? "true" : "false");
    option.className = "choice-group-option";
    option.textContent = label;
    group.appendChild(option);
  }
  document.body.appendChild(group);
  return group;
}

describe("ChoiceGroup sizes (live stylesheets)", () => {
  it("draws a horizontal option 26px high and never wraps its label", () => {
    proof.inject([".choice-group", ".choice-group-option"]);
    const option = mountGroup(false).querySelector<HTMLElement>(".choice-group-option")!;
    expect(getComputedStyle(option).height).toBe("26px");
    expect(getComputedStyle(option).whiteSpace).toBe("nowrap");
  });

  it("draws a vertical option 28px high, the list's row", () => {
    proof.inject([
      ".choice-group",
      ".choice-group-vertical",
      ".choice-group-option",
      ".choice-group-vertical .choice-group-option",
    ]);
    const option = mountGroup(true).querySelector<HTMLElement>(".choice-group-option")!;
    expect(getComputedStyle(option).height).toBe("28px");
  });

  it("never shrinks the segmented group, so its options cannot wrap under it", () => {
    proof.inject([".choice-group"]);
    const group = mountGroup(false);
    expect(getComputedStyle(group).display).toBe("inline-flex");
    expect(getComputedStyle(group).flexShrink).toBe("0");
  });
});
