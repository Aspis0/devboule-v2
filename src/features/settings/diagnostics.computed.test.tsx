// @vitest-environment happy-dom

// The Diagnostics page card language against the real stylesheets in bundle
// order: tokens, global, the shell sheet, then diagnostics.css. Bare
// single-class selectors in the light theme only (cssProof's scope); the
// dark theme and anything it cannot see belong to a live check and are
// listed in the slice report.

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
  read("src/features/settings/settings.css"),
  read("src/features/settings/diagnostics.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("diagnostics cards (real stylesheets, no app launch)", () => {
  it("grounds each report section on the house card", () => {
    proof.inject([".diagnostics-card"]);
    const card = box("diagnostics-card");
    const style = getComputedStyle(card);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    expect(proof.rulesFor(".diagnostics-card")).toContain(proof.token("--panel-card"));
    expect(proof.rulesFor(".diagnostics-card")).toContain(proof.token("--line"));
  });

  it("labels each card with the house section label", () => {
    proof.inject([".diagnostics-card-title"]);
    const title = document.createElement("h3");
    title.className = "diagnostics-card-title";
    document.body.appendChild(title);
    const style = getComputedStyle(title);
    expect(style.fontSize).toBe("12px");
    expect(style.fontWeight).toBe("500");
    expect(proof.rulesFor(".diagnostics-card-title")).toContain(proof.token("--muted"));
  });

  it("keeps the report sections on the shell's vertical rhythm", () => {
    expect(proof.rulesFor("#settings-panel-diagnostics > section")).toContain(
      "margin-bottom: 18px",
    );
  });

  it("sets report rows as label 14, sans", () => {
    proof.inject([".diagnostics-row dt"]);
    const rows = document.createElement("dl");
    rows.className = "diagnostics-rows";
    const row = document.createElement("div");
    row.className = "diagnostics-row";
    const key = document.createElement("dt");
    key.textContent = "uptime ms";
    row.appendChild(key);
    rows.appendChild(row);
    document.body.appendChild(rows);
    const style = getComputedStyle(key);
    expect(style.fontSize).toBe("14px");
    expect(style.fontFamily).not.toMatch(/monospace|JetBrains/i);
  });

  it("grounds the load-failure card like a report section", () => {
    expect(proof.rulesFor(".diagnostics-error")).toContain(proof.token("--panel-card"));
    expect(proof.rulesFor(".diagnostics-error")).toContain(proof.token("--line"));
  });

  it("rings keyboard focus on the copy and retry controls", () => {
    for (const target of [".diagnostics-copy:focus-visible", ".diagnostics-retry:focus-visible"]) {
      expect(proof.rulesFor(target)).toMatch(/outline:\s*2px solid/);
    }
  });
});

describe("diagnostics mono allowlist (real stylesheets)", () => {
  // Numbers, paths, versions and the raw report text are data, so mono
  // stays there. Everywhere else in this sheet it is a defect.
  it("keeps mono faces on values, the raw text and the retention input only", () => {
    const css = read("src/features/settings/diagnostics.css");
    const allowed = new Set([".diagnostics-row dd", ".diagnostics-text", ".retention-limit-input"]);
    for (const selector of monoSelectors(css)) {
      expect(allowed.has(selector), `mono face outside the allowlist: ${selector}`).toBe(true);
    }
    for (const selector of allowed) {
      expect(monoSelectors(css)).toContain(selector);
    }
  });
});

describe("journal retention cards (real stylesheets, no app launch)", () => {
  // The retention section renders on the Diagnostics page (general.css
  // says so at its head), so its cards live here, not in a second sheet.
  it("grounds the usage and limits blocks on the house card", () => {
    proof.inject([".retention-summary", ".retention-limits"]);
    for (const className of ["retention-summary", "retention-limits"]) {
      const block = box(className);
      const style = getComputedStyle(block);
      expect(style.maxWidth).toBe("720px");
      expect(style.borderRadius).toBe("12px");
      expect(proof.rulesFor(`.${className}`)).toContain(proof.token("--panel-card"));
      expect(proof.rulesFor(`.${className}`)).toContain(proof.token("--line"));
    }
  });
});
