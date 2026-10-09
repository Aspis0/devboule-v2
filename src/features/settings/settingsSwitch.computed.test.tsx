// @vitest-environment happy-dom

// The Settings switch against its live sheet, settingsSwitch.css. Bare
// single-class selectors in the light theme (cssProof's scope).

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

function luminance(hex: string): number {
  const channels = [1, 3, 5].map((at) => {
    const channel = parseInt(hex.slice(at, at + 2), 16) / 255;
    return channel <= 0.03928 ? channel / 12.92 : Math.pow((channel + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
}

/** WCAG contrast ratio of two `#rrggbb` colours. */
function contrastRatio(a: string, b: string): number {
  const [hi, lo] = luminance(a) > luminance(b) ? [a, b] : [b, a];
  return (luminance(hi) + 0.05) / (luminance(lo) + 0.05);
}

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("settings switch (live stylesheet)", () => {
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/settings.css"),
    read("src/features/settings/settingsSwitch.css"),
  ]);

  it("sizes the switch at 34x20 with the on fill", () => {
    proof.inject([".settings-switch", ".settings-switch-on"]);
    const toggle = box("settings-switch settings-switch-on");
    const style = getComputedStyle(toggle);
    expect(style.width).toBe("34px");
    expect(style.height).toBe("20px");
  });

  it("draws its keyboard focus ring in a colour that reads against the card", () => {
    // A ring on a fill of its own colour is invisible — exactly where
    // arrow-key focus starts. The ring and the card resolve through the
    // sheets' own tokens and must hold WCAG 3:1.
    const rules = proof.rulesFor(".settings-switch:focus-visible");
    const outline = rules
      .split(";")
      .map((part) => part.trim())
      .filter((part) => part.startsWith("outline:"))
      .at(-1);
    if (outline === undefined) throw new Error(`no outline in: ${rules}`);
    const value = outline.slice("outline:".length).trim();
    const ringToken = /var\((--[a-zA-Z0-9-]+)\)/.exec(value)?.[1];
    const ringHex = /#([0-9a-f]{6})/i.exec(value)?.[0];
    const ring = ringToken !== undefined ? proof.token(ringToken) : ringHex;
    const card = proof.token("--panel-card");
    if (ring === undefined || card === undefined) throw new Error(`unresolved ${value}`);
    expect(contrastRatio(ring, card), `${value} must hold 3:1 on the card`).toBeGreaterThanOrEqual(
      3,
    );
  });
});
