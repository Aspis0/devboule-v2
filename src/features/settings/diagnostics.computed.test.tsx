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

/** Every `--token: value` declaration under one selector, last wins. */
function tokenDeclarations(tokens: string, selector: string): Map<string, string> {
  const map = new Map<string, string>();
  const escaped = selector.replace(/[^a-z0-9]/gi, "\\$&");
  const pattern = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, "g");
  for (const match of tokens.matchAll(pattern)) {
    const body = (match[1] ?? "").replace(/\/\*[\s\S]*?\*\//g, "");
    for (const decl of body.matchAll(/--([a-zA-Z0-9-]+):\s*([^;]+);/g)) {
      map.set(`--${decl[1]}`, decl[2]!.trim());
    }
  }
  return map;
}

function resolveToken(
  root: Map<string, string>,
  dark: Map<string, string>,
  token: string,
  isDark: boolean,
): string {
  const decls = isDark ? new Map([...root, ...dark]) : root;
  let value = decls.get(token);
  if (value === undefined) throw new Error(`${token} not declared`);
  for (let pass = 0; pass < 4; pass += 1) {
    const ref = value.match(/^var\((--[a-z-]+)\)$/);
    if (ref === null) return value;
    const next = decls.get(ref[1]!);
    if (next === undefined) throw new Error(`unresolved ${value}`);
    value = next;
  }
  return value;
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

  it("lifts the in-card retry pill off the error card's ground", () => {
    // The devices sheet documents this exact fill-on-fill failure and its
    // fix (scoped --panel-side fill, --line-strong edge); the same shape
    // here, with the same floors: the fill must move at all, the edge
    // must carry the resting boundary.
    const css = read("src/features/settings/diagnostics.css");
    const override = css.match(/#settings-panel-diagnostics \.diagnostics-retry\s*\{([^}]*)\}/);
    if (override === null) throw new Error("scoped retry override not found");
    const tokenName = (prop: string): string => {
      const found = override[1]!.match(new RegExp(`${prop}:\\s*var\\((--[a-z-]+)\\)`));
      if (found === null) throw new Error(`${prop} token not found in override`);
      return found[1]!;
    };
    const fillToken = tokenName("background");
    const borderToken = tokenName("border-color");
    expect(fillToken).not.toBe("--surface");
    const tokens = read("src/styles/tokens.css");
    const root = tokenDeclarations(tokens, ":root");
    const dark = tokenDeclarations(tokens, '[data-theme="dark"]');
    for (const isDark of [false, true]) {
      const card = resolveToken(root, dark, "--panel-card", isDark);
      expect(contrastRatio(resolveToken(root, dark, fillToken, isDark), card)).toBeGreaterThan(1);
      expect(
        contrastRatio(resolveToken(root, dark, borderToken, isDark), card),
      ).toBeGreaterThanOrEqual(1.3);
    }
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
    // The heading, the two help sentences and the blocked-copy line are
    // direct children beside the rows; inset scoped per child kind (the
    // panel tsx belongs to another slice, so element hooks back the
    // class hooks below).
    for (const selector of [
      ".retention-summary > .settings-card",
      ".retention-limits > .retention-limit-row",
      ".retention-limits > .settings-subheading",
      ".retention-limits > .retention-help",
      ".retention-summary > .retention-blocked-copy",
    ]) {
      expect(proof.rulesFor(selector)).toContain("padding-left: 14px");
    }
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
      ".retention-limits > .settings-subheading",
      ".retention-limits > .retention-help",
    ]);
    const card = box("retention-limits");
    const heading = document.createElement("div");
    heading.className = "settings-subheading";
    const help = document.createElement("p");
    help.className = "retention-help";
    card.append(heading, help);
    expect(getComputedStyle(heading).marginBottom).toBe("8px");
    expect(getComputedStyle(help).marginBottom).toBe("8px");
  });
});
