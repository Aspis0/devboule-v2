// @vitest-environment happy-dom

// The This-machine card language against the real stylesheets. The sheet
// order below mirrors the bundle — tokens, global (main.tsx, static),
// general.css, then settings.css: SettingsSurface.tsx imports the Appearance
// section (line 7, which pulls general.css) ahead of "./settings.css" (line
// 23), and module evaluation follows declaration order. Scope, stated
// plainly: cssProof models bare selectors in the light theme only, and each
// `inject` carries only the named rules — so a same-named rule in another
// sheet is invisible unless named too. The one exception is the adversarial
// cascade case, which appends a synthetic later sheet on purpose and says so.

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

/** Every selector in the sheet whose rule sets a monospace family, @-blocks
 * included, so a responsive tweak cannot smuggle a mono face past the test. */
function monoSelectors(css: string): string[] {
  const found: string[] = [];
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const scan = (source: string): void => {
    let index = 0;
    while (index < source.length) {
      const open = source.indexOf("{", index);
      if (open < 0) return;
      const selector = source.slice(index, open);
      let depth = 1;
      let cursor = open + 1;
      while (depth > 0 && cursor < source.length) {
        if (source[cursor] === "{") depth += 1;
        if (source[cursor] === "}") depth -= 1;
        cursor += 1;
      }
      const body = source.slice(open + 1, cursor - 1);
      if (selector.trim().startsWith("@")) scan(body);
      else if (/JetBrains Mono|monospace/i.test(body)) {
        for (const part of selector.split(",")) found.push(part.trim().replace(/\s+/g, " "));
      }
      index = cursor;
    }
  };
  scan(stripped);
  return found;
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

describe("this-machine card language (real stylesheets, no app launch)", () => {
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/general.css"),
    read("src/features/settings/settings.css"),
    read("src/features/settings/settingsSwitch.css"),
  ]);

  it("holds the cards at max-width 720 with the r12 panel face", () => {
    proof.inject([".machine-card"]);
    const card = box("machine-card");
    const style = getComputedStyle(card);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    // The face itself, not just "something painted": the light --panel-card.
    expect(style.backgroundColor).toBe(proof.token("--panel-card"));
  });

  it("sets row titles at 14 with the description at 12 below", () => {
    proof.inject([".machine-row-title", ".machine-row-desc"]);
    expect(getComputedStyle(box("machine-row-title")).fontSize).toBe("14px");
    const desc = getComputedStyle(box("machine-row-desc"));
    expect(desc.fontSize).toBe("12px");
  });

  it("sizes the notification switch at 34x20 with the on fill", () => {
    proof.inject([".settings-switch", ".settings-switch-on"]);
    const toggle = box("settings-switch settings-switch-on");
    const style = getComputedStyle(toggle);
    expect(style.width).toBe("34px");
    expect(style.height).toBe("20px");
  });

  it("draws every focus ring in a colour that reads against its own fill", () => {
    // F3: a ring on a fill of its own colour is invisible — exactly where
    // arrow-key focus starts. Each pair below resolves both sides through
    // the sheets' own tokens and demands WCAG 3:1, so `transparent` or a
    // same-fill colour fails instead of passing a text scan.
    const ringVsFill: Array<[ringRules: string, ringProp: string, fillToken: string]> = [
      [proof.rulesFor(".machine-segment-option-checked:focus-within"), "outline-color", "--ink"],
      [proof.rulesFor(".machine-segment-option:focus-within"), "outline", "--panel-card"],
      [proof.rulesFor(".settings-switch:focus-visible"), "outline", "--panel-card"],
    ];
    for (const [rules, prop, fillToken] of ringVsFill) {
      const match = rules
        .split(";")
        .map((part) => {
          const colon = part.indexOf(":");
          return colon === -1
            ? null
            : { name: part.slice(0, colon).trim(), value: part.slice(colon + 1).trim() };
        })
        .filter(
          (parsed): parsed is { name: string; value: string } =>
            parsed !== null && parsed.name === prop,
        )
        .at(-1);
      if (match === undefined) throw new Error(`no ${prop} in: ${rules}`);
      // cssProof resolves `var()` at assembly, so the value arrives as a
      // token reference or an already-resolved hex inside the shorthand —
      // both trace to the sheets' own tokens, neither may be hand-written.
      const ringToken = /var\((--[a-zA-Z0-9-]+)\)/.exec(match.value)?.[1];
      const ringHex = /#([0-9a-f]{6})/i.exec(match.value)?.[0];
      const ring = ringToken !== undefined ? proof.token(ringToken) : ringHex;
      const fill = proof.token(fillToken);
      if (ring === undefined || fill === undefined) {
        throw new Error(`unresolved ${ringToken} or ${fillToken}`);
      }
      expect(
        contrastRatio(ring, fill),
        `${ringToken ?? match.value} (${ring}) on ${fillToken} (${fill}) must hold 3:1`,
      ).toBeGreaterThanOrEqual(3);
    }
  });

  it("keeps this page's own sheet free of any mono face", () => {
    // No mono may ever be declared here: the cross-sheet allowlist (which
    // sheets may carry mono, and on which selectors) lives in
    // settingsStatic.test.ts. This line fails the moment general.css grows
    // one, whichever @-block it hides in.
    expect(monoSelectors(read("src/features/settings/general.css"))).toEqual([]);
  });

  it("holds the radio colour by specificity, not by sheet order", () => {
    // F7: `.machine-choice input` ties `global.css input[type="radio"]` at
    // (0,1,1), so a later sheet with a different value would win. The
    // synthetic last sheet below stands in for any such future rule: the
    // machine rule must still win after the specificity fix.
    const adversarial = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/settings/general.css"),
      'input[type="radio"] { accent-color: #123456; }',
    ]);
    adversarial.inject([".machine-choices .machine-choice input", 'input[type="radio"]']);
    const list = document.createElement("div");
    list.className = "machine-choices";
    const choice = document.createElement("label");
    choice.className = "machine-choice";
    const input = document.createElement("input");
    input.type = "radio";
    choice.appendChild(input);
    list.appendChild(choice);
    document.body.appendChild(list);
    const style = getComputedStyle(input);
    expect((style as unknown as { accentColor: string }).accentColor).toBe(
      adversarial.token("--ink"),
    );
  });
});
