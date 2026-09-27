// @vitest-environment happy-dom

// The Devices card language against the real stylesheets in bundle order.
// The assembly is tokens, global, devices.css, settings.css — the sheets
// that carry every `dev-` rule (no other settings sheet declares one, and
// the test below fails if a later slice adds one elsewhere). It is not the
// whole chunk: providers, profiles, general and diagnostics sheets also
// load, so a same-named rule there would still win a real cascade.
// Scope, stated plainly: cssProof models bare single-class selectors in the
// light theme only (see its header) — this suite proves the rules exist with
// the spec's values, not the rendered cascade. Anything it cannot see
// (flex line-breaking, the dark theme, descendant conflicts) belongs to a
// live check and is listed in the slice report.
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

/** Every selector in the sheet whose rule sets a monospace family,
 * including rules nested inside `@`-blocks (a responsive tweak must not
 * smuggle a mono face past the allowlist). */
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

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

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

/**
 * One token's resolved value for one theme. `:root` declarations apply to
 * both themes and `[data-theme="dark"]` overlays them — so an alias
 * declared once (like `--terracotta`, or `--surface-muted`) re-resolves
 * through whichever theme's chain names it.
 */
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

interface StyleRule {
  selector: string;
  body: string;
}

/** Flat rules in source order; `@`-blocks splice their inner rules inline. */
function styleRules(source: string): StyleRule[] {
  const rules: StyleRule[] = [];
  let index = 0;
  while (index < source.length) {
    const open = source.indexOf("{", index);
    if (open < 0) return rules;
    const selector = source.slice(index, open).trim();
    let depth = 1;
    let cursor = open + 1;
    while (depth > 0 && cursor < source.length) {
      if (source[cursor] === "{") depth += 1;
      if (source[cursor] === "}") depth -= 1;
      cursor += 1;
    }
    const body = source.slice(open + 1, cursor - 1);
    if (selector.startsWith("@")) {
      for (const inner of styleRules(body)) rules.push(inner);
    } else if (selector !== "") {
      rules.push({ selector: selector.replace(/\s+/g, " "), body });
    }
    index = cursor;
  }
  return rules;
}

/** Specificity as [ids, classes, elements]; pseudo-classes count as classes. */ function selectorSpecificity(
  selector: string,
): [number, number, number] {
  const flat = selector.replace(/::[a-z-]+/g, " ");
  const ids = (flat.match(/#[a-zA-Z0-9_-]+/g) ?? []).length;
  const classes =
    (flat.match(/\.[a-zA-Z0-9_-]+/g) ?? []).length +
    (flat.match(/\[[^\]]+\]/g) ?? []).length +
    (flat.match(/:(?!:)[a-z-]+/g) ?? []).length;
  const elements = (flat.match(/(^|[\s>+~])([a-zA-Z][a-zA-Z0-9-]*)/g) ?? []).length;
  return [ids, classes, elements];
}

/** The `var()` token colouring one rule's border, if it declares one. */
function ruleBorderToken(body: string): string | null {
  const longhand = body.match(/border-color\s*:\s*var\((--[a-z-]+)\)/);
  if (longhand !== null) return longhand[1]!;
  const shorthand = body.match(/(?:^|;)\s*border\s*:[^;]*var\((--[a-z-]+)\)/);
  return shorthand?.[1] ?? null;
}

/**
 * The border-colour token that wins for one interactive state, replaying
 * the cascade by hand: highest specificity, source order breaking ties.
 */
function winningBorderToken(rules: StyleRule[], state: string): string | null {
  const rank = (a: [number, number, number], b: [number, number, number]): number =>
    a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
  // A holder, never null itself: every real specificity beats (-1,-1,-1),
  // so the first candidate always installs and no null-narrowing is needed.
  const best: { spec: [number, number, number]; index: number; token: string | null } = {
    spec: [-1, -1, -1],
    index: -1,
    token: null,
  };
  rules.forEach((rule, index) => {
    const token = ruleBorderToken(rule.body);
    if (token === null) return;
    for (const part of rule.selector.split(",")) {
      const trimmed = part.trim();
      if (!trimmed.includes(".settings-device-action")) continue;
      const pseudos = trimmed.match(/:(?!:)[a-z-]+/g) ?? [];
      if (!pseudos.every((pseudo) => pseudo === `:${state}`)) continue;
      const spec = selectorSpecificity(trimmed);
      if (rank(spec, best.spec) > 0 || (rank(spec, best.spec) === 0 && index > best.index)) {
        best.spec = spec;
        best.index = index;
        best.token = token;
      }
    }
  });
  return best.token;
}

describe("shared form rules live in the shell sheet (real stylesheets)", () => {
  // Eight components (ProfileRow, ProfileDialog, AgentProfileForm,
  // AgentProfileFeatures, AgentProfileOverlay, AgentProfileVocabulary,
  // AgentsPanel, ProviderNpmFailure) render `device-*` classes without
  // importing any stylesheet: the rules live in `settings.css`, which the
  // shell always loads, so no page chunk can strand them. These tests prove
  // the geometry through the cascade, not through a substring match.
  const shell = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/settings.css"),
  ]);

  function fieldInput(): HTMLInputElement {
    const label = document.createElement("label");
    label.className = "device-field";
    const input = document.createElement("input");
    label.appendChild(input);
    document.body.appendChild(label);
    return input;
  }

  it("gives the shared inline confirm its box", () => {
    shell.inject([".device-inline-confirm"]);
    const style = getComputedStyle(box("device-inline-confirm"));
    expect(style.display).toBe("grid");
    expect(style.paddingTop).toBe("8px");
    expect(style.paddingLeft).toBe("10px");
    expect(style.borderRadius).toBe("10px");
    // The border must exist and match the shell's own action button —
    // compared resolved, so no hex is duplicated into this file.
    const borderColor = (body: string): string => {
      const found = body.match(/1px solid ([^;]+);/);
      if (found === null) throw new Error("no 1px solid border");
      return found[1]!.trim();
    };
    expect(borderColor(shell.rulesFor(".device-inline-confirm"))).toBe(
      borderColor(shell.rulesFor(".settings-device-action")),
    );
  });

  it("gives the shared field inputs their box, with no mono face", () => {
    shell.inject([".device-field", ".device-field input"]);
    const style = getComputedStyle(fieldInput());
    expect(style.fontSize).toBe("11.5px");
    expect(style.borderRadius).toBe("8px");
    expect(style.fontFamily).not.toMatch(/monospace|JetBrains/i);
    expect(shell.rulesFor(".device-field input")).not.toMatch(/monospace|JetBrains/i);
  });

  it("leaves the shared rules to the shell sheet alone", () => {
    // Single owner: if these selectors ever drift back into the page
    // sheet, two sources style the same classes and the shell proof
    // above stops describing the bundle.
    const css = read("src/features/settings/devices.css");
    for (const selector of [
      ".device-copy",
      ".device-field",
      ".device-field-hint",
      ".device-error",
      ".device-actions",
      ".device-inline-confirm",
    ]) {
      expect(css, selector).not.toContain(`${selector} {`);
    }
  });
});

describe("settings row names at a 64-character maximum (real stylesheets)", () => {
  // The daemon accepts a 64-character display name
  // (`MAX_DISPLAY_NAME_CHARS`), so the name column must give way before
  // the status and the kebab do. cssProof has no layout engine: what it
  // pins is the mechanism — a zero flex minimum on the name, without which
  // `white-space: nowrap` pins the item at its full text width and the
  // ellipsis never engages. Whether 64 real characters overflow 720 px
  // needs a browser and is listed in the slice report.
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/devices.css"),
    read("src/features/settings/settings.css"),
  ]);

  function deviceRowWithName(text: string): HTMLElement {
    const card = box("dev-card");
    card.style.width = "720px";
    const row = document.createElement("div");
    row.className = "dev-row";
    const name = document.createElement("span");
    name.className = "dev-name";
    name.textContent = text;
    const status = document.createElement("span");
    status.className = "dev-status";
    status.textContent = "online";
    row.append(name, status);
    card.appendChild(row);
    return name;
  }

  it("lets the devices name shrink so the ellipsis can engage", () => {
    proof.inject([".dev-card", ".dev-row", ".dev-name", ".dev-status"]);
    const name = deviceRowWithName("x".repeat(64));
    const style = getComputedStyle(name);
    expect(style.minWidth).toBe("0");
    expect(style.overflow).toBe("hidden");
    expect(style.textOverflow).toBe("ellipsis");
    expect(style.whiteSpace).toBe("nowrap");
    expect(getComputedStyle(name.parentElement!).display).toBe("flex");
  });

  it("lets the providers name shrink the same way", () => {
    // The same defect R17-1 shipped in `.prov-name`: fixed house-wide so
    // the next page does not ship it again.
    const providerProof = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/settings/providers.css"),
      read("src/features/settings/settings.css"),
    ]);
    providerProof.inject([".prov-row", ".prov-name", ".prov-status"]);
    const row = document.createElement("div");
    row.className = "prov-row";
    const name = document.createElement("span");
    name.className = "prov-name";
    name.textContent = "x".repeat(64);
    row.appendChild(name);
    document.body.appendChild(row);
    const style = getComputedStyle(name);
    expect(style.minWidth).toBe("0");
    expect(style.overflow).toBe("hidden");
    expect(style.textOverflow).toBe("ellipsis");
    expect(getComputedStyle(row).display).toBe("flex");
  });
});

describe("devices card geometry (real stylesheets, no app launch)", () => {
  // Sheet order matches the bundle: tokens, global (main.tsx, static), then
  // devices.css BEFORE settings.css — SettingsSurface.tsx imports the
  // DevicesPanel (line 5) ahead of "./settings.css" (line 23), and module
  // evaluation follows declaration order.
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/devices.css"),
    read("src/features/settings/settings.css"),
  ]);

  it("holds page cards at max-width 720, r12, on the card ground", () => {
    proof.inject([".dev-card", ".settings-main-inner"]);
    const card = box("dev-card");
    const style = getComputedStyle(card);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    expect(proof.rulesFor(".dev-card")).toContain(proof.token("--panel-card"));
    // Not a containment proof (neither element is inside the other): it
    // fails loudly if the column's 720 ever moves away from the cards'.
    const column = getComputedStyle(box("settings-main-inner")).maxWidth;
    expect(style.maxWidth).toBe(column);
  });

  it("divides stacked rows with the line token", () => {
    expect(proof.rulesFor(".dev-row-wrap + .dev-row-wrap")).toContain(proof.token("--line"));
  });

  it("sets row names at 14px", () => {
    proof.inject([".dev-name"]);
    expect(getComputedStyle(box("dev-name")).fontSize).toBe("14px");
  });

  it("sets row meta and status at 12px", () => {
    proof.inject([".dev-meta", ".dev-status"]);
    expect(getComputedStyle(box("dev-meta")).fontSize).toBe("12px");
    expect(getComputedStyle(box("dev-status")).fontSize).toBe("12px");
  });

  it("keeps the status line at 12 with the live dot on the live tone", () => {
    proof.inject([".dev-status", ".dev-dot-live"]);
    expect(getComputedStyle(box("dev-status")).fontSize).toBe("12px");
    expect(proof.rulesFor(".dev-dot-live")).toContain(proof.token("--tone-live"));
  });

  it("gives the glyph its own 14px box", () => {
    proof.inject([".dev-glyph"]);
    const glyph = box("dev-glyph");
    expect(getComputedStyle(glyph).width).toBe("14px");
    expect(getComputedStyle(glyph).height).toBe("14px");
  });

  it("sizes the row kebab at 26px like the house icon buttons", () => {
    proof.inject([".dev-kebab"]);
    const kebab = box("dev-kebab");
    expect(getComputedStyle(kebab).width).toBe("26px");
    expect(getComputedStyle(kebab).height).toBe("26px");
  });

  it("owns every dev- rule: no other settings sheet declares one", () => {
    // What makes the four-sheet assembly above sound. A `dev-*` selector
    // in providers, profiles, general or diagnostics would join the real
    // cascade invisibly to every other test in this file.
    for (const sheet of [
      "src/features/settings/providers.css",
      "src/features/settings/profiles.css",
      "src/features/settings/general.css",
      "src/features/settings/diagnostics.css",
    ]) {
      const selectors = read(sheet)
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .match(/\.[a-z][a-z0-9-]*/g);
      expect(selectors?.filter((s) => s.startsWith(".dev-")) ?? [], sheet).toEqual([]);
    }
  });

  it("keeps mono type to code on every settings sheet", () => {
    // N5: the risk moved sheets once already (the shared `device-*` rules
    // now live in the shell sheet), so the allowlist scans all six. Each
    // entry names its owner: devices (this slice), shell legacy + shell
    // model picker (R17-0), providers (R17-1's own guard mirrors it),
    // retention + diagnostics rows + the raw report text (diagnostics
    // sheet; R17-5 declares the `pre` face explicitly instead of relying
    // on the user-agent default).
    const sheets: Record<string, readonly string[]> = {
      "src/features/settings/devices.css": [
        ".dev-pair-code",
        ".dev-fingerprint",
        ".dev-typed-input",
      ],
      "src/features/settings/settings.css": [
        // Legacy meta lines still rendered by other slices' panels, and
        // the shell's own model picker control: declared, not refactored.
        ".settings-card-meta",
        ".settings-card-value",
        ".model-choice-control",
      ],
      "src/features/settings/providers.css": [
        ".prov-detail-code",
        ".provider-consent-command",
        ".provider-update-error pre",
        ".provider-version",
      ],
      "src/features/settings/profiles.css": [],
      "src/features/settings/general.css": [],
      "src/features/settings/diagnostics.css": [
        ".retention-limit-input",
        ".diagnostics-row dd",
        ".diagnostics-text",
      ],
    };
    let scanned = 0;
    for (const [sheet, allowed] of Object.entries(sheets)) {
      const seen = monoSelectors(read(sheet));
      scanned += seen.length;
      expect(
        seen.filter((selector) => !allowed.includes(selector)),
        sheet,
      ).toEqual([]);
    }
    // The scanner is alive: it found mono faces somewhere.
    expect(scanned).toBeGreaterThan(0);
  });

  it("leaves the shared field rule without a mono face", () => {
    // `.device-field input` styles the Agents page's Name and Icon fields
    // too — neither is read one character at a time, so the shared rule
    // carries padding and border only. The pairing inputs take their mono
    // from their own class.
    expect(proof.rulesFor(".device-field input")).not.toMatch(/monospace|JetBrains/i);
    expect(proof.rulesFor(".dev-typed-input")).toMatch(/monospace/);
  });

  it("locks the button/card separation against regression", () => {
    // Regression tripwire, not WCAG 1.4.11 conformance (3 : 1): the resting
    // fill sits at 1.06 and the resting border at 1.7 / 1.6, both under the
    // standard. The conformant boundary is the hover/focus terracotta
    // (4.7 / 5.1, pinned below). What this gates is flattening: the fill
    // floor locks the override (the base is exactly 1.00), the border floor
    // locks the stronger resting edge. No quiet fill token reaches 1.3 in
    // both themes (panel-side is 1.063/1.076, fill-hover 1.153/1.21), so the
    // border carries the resting boundary and the test says so.
    const devices = read("src/features/settings/devices.css");
    const override = devices.match(
      /#settings-panel-devices \.settings-device-action\s*\{([^}]*)\}/,
    );
    if (override === null) throw new Error("scoped button override not found");
    const tokenName = (prop: string): string => {
      const found = override[1]!.match(new RegExp(`${prop}:\\s*var\\((--[a-z-]+)\\)`));
      if (found === null) throw new Error(`${prop} token not found in override`);
      return found[1]!;
    };
    const fillToken = tokenName("background");
    const borderToken = tokenName("border-color");
    const tokens = read("src/styles/tokens.css");
    const root = tokenDeclarations(tokens, ":root");
    const dark = tokenDeclarations(tokens, '[data-theme="dark"]');
    for (const isDark of [false, true]) {
      const card = resolveToken(root, dark, "--panel-card", isDark);
      expect(
        contrastRatio(resolveToken(root, dark, fillToken, isDark), card),
      ).toBeGreaterThanOrEqual(1.05);
      expect(
        contrastRatio(resolveToken(root, dark, borderToken, isDark), card),
      ).toBeGreaterThanOrEqual(1.3);
    }
  });

  it("keeps the house hover and focus border on the card buttons", () => {
    // N1: the id-scoped override (1,1,0) outranks the house `:hover` /
    // `:focus-visible` rules (0,2,0), so without id-scoped states the
    // terracotta border never renders on this page. happy-dom has no
    // pseudo-class state, so this replays the cascade by hand over the
    // assembled sheets in order: every rule that can colour the pill in
    // each state, ranked by specificity with source order breaking ties.
    const joined = [
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/settings/devices.css"),
      read("src/features/settings/settings.css"),
    ]
      .map((sheet) => sheet.replace(/\/\*[\s\S]*?\*\//g, ""))
      .join("\n");
    const rules = styleRules(joined);
    for (const state of ["hover", "focus-visible"]) {
      const token = winningBorderToken(rules, state);
      if (token === null) throw new Error(`no border rule for :${state}`);
      expect(token, `:${state}`).toBe("--terracotta");
    }
    // And the winning token earns the conformance the resting state
    // lacks: terracotta against the card clears 3 : 1 in both themes.
    // Resolved per theme through the merged declarations, because the
    // alias is declared once and re-resolves (as `--surface-muted` does).
    const tokens = read("src/styles/tokens.css");
    const root = tokenDeclarations(tokens, ":root");
    const dark = tokenDeclarations(tokens, '[data-theme="dark"]');
    for (const isDark of [false, true]) {
      expect(
        contrastRatio(
          resolveToken(root, dark, "--terracotta", isDark),
          resolveToken(root, dark, "--panel-card", isDark),
        ),
      ).toBeGreaterThanOrEqual(3);
    }
  });

  it("keeps the button fill distinct from the card ground in both themes", () => {
    // Premise guard for the override above: it fills `--panel-side`, which
    // must resolve away from `--panel-card` light and dark (`--surface-muted`
    // resolves to the same colour in both themes, so either name fills
    // identically). Values come from the token sheet, not from constants
    // in this file.
    const tokens = read("src/styles/tokens.css");
    const root = tokenDeclarations(tokens, ":root");
    const dark = tokenDeclarations(tokens, '[data-theme="dark"]');
    for (const isDark of [false, true]) {
      expect(resolveToken(root, dark, "--panel-side", isDark)).not.toBe(
        resolveToken(root, dark, "--panel-card", isDark),
      );
    }
  });

  it("paints checkboxes and radios in the app accent, house-wide", () => {
    // Live check: the pairing role radios rendered in the browser's
    // default blue. One inherited line in the global sheet fixes every
    // checkbox and radio in both themes.
    proof.inject(['input[type="checkbox"]', 'input[type="radio"]']);
    for (const kind of ["checkbox", "radio"]) {
      const control = document.createElement("input");
      control.type = kind;
      document.body.appendChild(control);
      expect(getComputedStyle(control).accentColor).toBe(proof.token("--accent"));
    }
  });

  it("sets card titles at the spec's 12px section-label size", () => {
    // SPEC-regions: section labels 12/500 muted. The titles sit inside
    // the card heads (with the intro copy they belong to), so the device's
    // own 14px name below keeps an emphasis of its own.
    proof.inject([".dev-card-title", ".dev-meta"]);
    const title = box("dev-card-title");
    expect(getComputedStyle(title).fontSize).toBe("12px");
    expect(getComputedStyle(title).fontWeight).toBe("500");
    // Muted, proved without a hex: the same computed colour as the
    // house meta line.
    expect(getComputedStyle(title).color).toBe(getComputedStyle(box("dev-meta")).color);
  });

  it("sets the pairing role legend in sentence case like every other label", () => {
    // Live check: "PAIR THE OTHER DEVICE AS" was the only all-caps label
    // on the page. The JSX already reads "Pair the other device as"; the
    // caps came from this rule, now a 12/500 label with no transform.
    proof.inject([".dev-role-choice legend"]);
    const boxEl = box("dev-role-choice");
    const legend = document.createElement("legend");
    boxEl.appendChild(legend);
    const style = getComputedStyle(legend);
    expect(style.fontSize).toBe("12px");
    expect(style.fontWeight).toBe("500");
    expect(style.textTransform).not.toBe("uppercase");
  });

  it("keeps the first card on the shell's 18px rhythm", () => {
    // Our sections sit inside #settings-panel-devices, out of reach of
    // the shell's `.settings-main-inner > section` rule, so the reference
    // is restated here. `rulesFor` matches the exact selector, combinator
    // included — cssProof cannot inject it, but it can read it.
    expect(proof.rulesFor("#settings-panel-devices > section")).toContain("margin-bottom: 18px");
  });

  it("lays revoked rows out as rows, aligned under the paired names", () => {
    // The revoked list carries no glyph, so the rows align their names
    // under the paired names (38px: 14 pad + 14 glyph + 10 gap) with
    // their own padding — the `<summary>` disclosure label stays at the
    // card's 14px edge.
    proof.inject([".dev-revoked", ".dev-revoked-row"]);
    const row = box("dev-revoked-row");
    expect(getComputedStyle(row).display).toBe("flex");
    expect(getComputedStyle(row).paddingLeft).toBe("24px");
    expect(proof.rulesFor(".dev-revoked")).not.toContain("padding-left");
  });
});
