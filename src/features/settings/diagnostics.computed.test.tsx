// @vitest-environment happy-dom

// The Diagnostics page card language against the real stylesheets in the
// REAL bundle order, measured with `vite build` on this tree
// (SettingsSurface chunk byte offsets: diagnostics 55, devices 3427,
// oracle 7656, general 25685, providers 26993, profiles 33013, projects
// 38259, settings.css LAST at 39720+). Bare single-class selectors in the
// light theme only (cssProof's scope); the dark theme and anything it
// cannot see belong to a live check and are listed in the slice report.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

/** Every selector in the sheet whose rule sets a monospace family. */
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

describe("diagnostics page (real stylesheets, no app launch)", () => {
  it("leaves the raw report's typeface to the pre default", () => {
    // The pasted-into-an-issue block keeps rendering exactly as base had
    // it: no font-family declaration anywhere on its selector.
    expect(proof.rulesFor(".diagnostics-text")).not.toMatch(/font-family/);
  });

  it("moves the pill edge on hover and focus without owning the ring", () => {
    // The ring ships app-wide (global.css `button:focus-visible`); this
    // sheet only moves colour and edge, like the other settings sheets.
    for (const target of [".diagnostics-copy:hover", ".diagnostics-retry:hover"]) {
      // --terracotta is an alias for --accent; token() reads one level,
      // so the assertion names the hex directly.
      expect(proof.rulesFor(target)).toContain(proof.token("--accent"));
    }
    expect(read("src/features/settings/diagnostics.css")).not.toMatch(
      /diagnostics-(copy|retry):focus-visible\s*\{[^}]*outline/,
    );
  });
});

describe("diagnostics mono allowlist (real stylesheets)", () => {
  // Numbers, paths and versions are data, so mono stays on the values and
  // the retention input. Everywhere else in this sheet it is a defect.
  // The raw report block is deliberately absent: it keeps the `pre`
  // default exactly as base had it, so no declaration may name it.
  it("keeps mono faces on values and the retention input only", () => {
    const css = read("src/features/settings/diagnostics.css");
    const allowed = new Set([".retention-limit-input"]);
    for (const selector of monoSelectors(css)) {
      expect(allowed.has(selector), `mono face outside the allowlist: ${selector}`).toBe(true);
    }
    for (const selector of allowed) {
      expect(monoSelectors(css)).toContain(selector);
    }
  });
});
