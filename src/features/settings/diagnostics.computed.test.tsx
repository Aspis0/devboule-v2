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
      "margin-bottom: 16px",
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

  it("leaves the raw report's typeface to the pre default", () => {
    // The pasted-into-an-issue block keeps rendering exactly as base had
    // it: no font-family declaration anywhere on its selector.
    expect(proof.rulesFor(".diagnostics-text")).not.toMatch(/font-family/);
  });

  it("holds the safety note off the first report card", () => {
    proof.inject([".diagnostics-note"]);
    const note = document.createElement("p");
    note.className = "diagnostics-note";
    document.body.appendChild(note);
    expect(getComputedStyle(note).marginBottom).toBe("14px");
  });

  it("grounds the load-failure card like a report section", () => {
    expect(proof.rulesFor(".diagnostics-error")).toContain(proof.token("--panel-card"));
    expect(proof.rulesFor(".diagnostics-error")).toContain(proof.token("--line"));
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
  // Numbers, paths and versions are data, so mono stays on the row values
  // and the retention input. Everywhere else in this sheet it is a defect.
  // The raw report block is deliberately absent: it keeps the `pre`
  // default exactly as base had it, so no declaration may name it.
  it("keeps mono faces on values and the retention input only", () => {
    const css = read("src/features/settings/diagnostics.css");
    const allowed = new Set([".diagnostics-row dd", ".retention-limit-input"]);
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

  it("insets every card child — rows and prose alike — at 14px", () => {
    // Structural selectors keep the inset independent of the child classes.
    for (const selector of [
      ".retention-summary > .settings-card",
      ".retention-limits > .retention-limit-row",
      ".retention-limits > *",
      ".retention-summary > .retention-blocked-copy",
    ]) {
      expect(proof.rulesFor(selector)).toContain("padding-left: 14px");
    }
    expect(proof.rulesFor(".retention-limits > :first-child")).toContain("margin: 0 0 8px");
  });

  it("divides retention rows on the house line", () => {
    for (const selector of [
      ".retention-summary > div + div",
      ".retention-limits > label + label",
    ]) {
      expect(proof.rulesFor(selector)).toContain(proof.token("--line"));
    }
  });

  it("keeps the prose gaps inside the limits card", () => {
    proof.inject([
      ".retention-limits > *",
      ".retention-limits > :first-child",
      ".retention-limits > .retention-help",
      ".settings-subheading",
    ]);
    const card = box("retention-limits");
    const heading = document.createElement("div");
    heading.className = "settings-subheading";
    const help = document.createElement("p");
    help.className = "retention-help";
    card.append(heading, help);
    expect(getComputedStyle(heading).marginTop).toBe("0px");
    expect(getComputedStyle(heading).marginBottom).toBe("8px");
    expect(getComputedStyle(help).marginBottom).toBe("8px");
  });
});
