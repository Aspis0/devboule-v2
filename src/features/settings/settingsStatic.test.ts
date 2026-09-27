import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { SETTINGS_MENU } from "./settingsMenu";

// Static contracts on the settings stylesheets and catalogue: no rendering,
// no root, no afterEach. Each guard parses the declaration it pins, so a
// mutation run fails on the assertion below rather than on a hook.

/** Every `outline`/`outline-style` value declared for the selector, in sheet
    order: the cascade winner is last, and a killing override anywhere in the
    list must fail the guard. */
function outlineValues(css: string, selector: string): string[] {
  const out: string[] = [];
  let from = 0;
  while (true) {
    const start = css.indexOf(selector, from);
    if (start === -1) break;
    const open = css.indexOf("{", start);
    const close = open === -1 ? -1 : css.indexOf("}", open);
    if (open === -1 || close === -1) break;
    for (const declaration of css.slice(open + 1, close).split(";")) {
      const colon = declaration.indexOf(":");
      if (colon === -1) continue;
      const prop = declaration.slice(0, colon).trim();
      if (prop === "outline" || prop === "outline-style") {
        out.push(declaration.slice(colon + 1).trim());
      }
    }
    from = close + 1;
  }
  return out;
}

function settingsCss(name: string): string {
  return readFileSync(resolve(import.meta.dirname, name), "utf8");
}

function settingsTsx(name: string): string {
  return readFileSync(resolve(import.meta.dirname, name), "utf8");
}

/** True when the sheet defines a rule for exactly this selector — a scoped
 * accommodation like `.card > .name:first-child` does not count. */
function definesBareRule(css: string, selector: string): boolean {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const escaped = selector.replace(/[^a-z0-9]/gi, "\\$&");
  return new RegExp(`(^|[,}])\\s*${escaped}\\s*\\{`).test(stripped);
}

/** Every selector in the sheet whose rule sets a monospace family, @-blocks
 * included. The same walker the computed suites use, so a responsive tweak
 * cannot smuggle a mono face past the allowlist below. */
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

describe("Settings static contracts", () => {
  it("rings keyboard focus on the menu rows with a drawn outline", () => {
    const outlines = outlineValues(settingsCss("settings.css"), ".settings-menu-row:focus-visible");
    expect(outlines.length).toBeGreaterThan(0);
    for (const outline of outlines) {
      expect(outline).not.toMatch(/^(none|0|transparent)$/i);
      const tokens = outline.split(/\s+/).map((token) => token.toLowerCase());
      expect(
        tokens.some((token) =>
          ["solid", "dotted", "dashed", "double", "groove", "ridge", "inset", "outset"].includes(
            token,
          ),
        ),
      ).toBe(true);
    }
  });

  it("keeps its screen-reader utility in the global styles, not in settings", () => {
    const global = settingsCss("../../styles/global.css");
    const shell = settingsCss("settings.css");
    expect(global).toContain(".sr-only");
    expect(shell).not.toContain(".sr-only");
  });

  it("defines section labels once, at the spec values", () => {
    const shell = settingsCss("settings.css");
    const profiles = settingsCss("profiles.css");
    const diagnostics = settingsCss("diagnostics.css");
    const projects = settingsCss("projects.css");
    // The section-label scale has one owner. Page sheets use structural
    // selectors for spacing so none can fork the label rule.
    for (const css of [profiles, diagnostics, projects]) {
      expect(css.replace(/\/\*[\s\S]*?\*\//g, "")).not.toContain(".settings-subheading");
    }
    // general.css may borrow the shell's label for its card heads, never
    // re-declare it: a scoped spacing accommodation is not a definition.
    const general = settingsCss("general.css");
    expect(general).toContain(".settings-subheading");
    expect(definesBareRule(general, ".settings-subheading")).toBe(false);
    const rule = /\.settings-subheading\s*\{([^}]*)\}/.exec(shell)?.[1] ?? "";
    expect(rule).toContain("font-size: 12px");
    expect(rule).toContain("font-weight: 500");
  });

  it("zeroes the card-head margins in the author rule, never relying on UA defaults", () => {
    // N2: happy-dom ships no UA sheet, so no computed style can see the
    // h3's margin-block-end — the guard asserts the author rule that
    // zeroes it. One block head per card plus explicit zeroes is what
    // makes the Appearance and Editing heads render identical spacing.
    const general = settingsCss("general.css");
    const body =
      /\.machine-card\s*>\s*\.settings-subheading:first-child\s*\{([^}]*)\}/.exec(general)?.[1] ??
      "";
    expect(body, "the head accommodation rule is missing").not.toBe("");
    expect(body).toMatch(/margin\s*:\s*0|margin-block-end\s*:\s*0/);
  });

  it("carries no styling ghost classes on the This-machine markup", () => {
    // F5: eleven class names were left in the JSX with zero CSS rules,
    // kept alive only as test selectors. The selectors below read semantic
    // hooks (input names, roles, the shared card classes) instead. The scan
    // reads className tokens only, so `name="send-behavior"` (a live form
    // name, not a style hook) does not count.
    const markup = settingsTsx("AppearanceSection.tsx") + settingsTsx("SendBehaviorSetting.tsx");
    const tokens = new Set<string>();
    for (const found of markup.matchAll(/className="([^"]*)"/g)) {
      for (const token of (found[1] ?? "").split(/\s+/)) tokens.add(token);
    }
    for (const ghost of [
      "appearance-section",
      "appearance-options",
      "appearance-option",
      "appearance-option-copy",
      "appearance-option-label",
      "appearance-option-hint",
      "appearance-persist-note",
      "send-behavior",
      "send-behavior-options",
      "send-behavior-option",
      "send-behavior-note",
    ]) {
      expect(tokens, `${ghost} has no CSS rule and must leave the markup`).not.toContain(ghost);
    }
  });

  it("carries mono only where the house allows it", () => {
    // F6: every settings sheet scanned, @-blocks included, against an
    // explicit allowlist. general.css is not on it: no mono may ever be
    // declared on a This-machine page. A new mono face anywhere else fails
    // here until its selector is declared — and justified — below.
    const sheets = [
      "settings.css",
      "general.css",
      "providers.css",
      "profiles.css",
      "devices.css",
      "diagnostics.css",
    ] as const;
    const allowlist = new Map<string, readonly string[]>([
      ["settings.css", [".settings-card-meta", ".model-choice-control", ".settings-card-value"]],
      ["general.css", []],
      [
        "providers.css",
        [
          ".prov-detail-code",
          ".provider-consent-command",
          ".provider-update-error pre",
          ".provider-version",
        ],
      ],
      ["profiles.css", []],
      ["devices.css", [".dev-fingerprint", ".dev-pair-code", ".dev-typed-input"]],
      ["diagnostics.css", [".retention-limit-input", ".diagnostics-row dd"]],
    ]);
    for (const sheet of sheets) {
      expect(
        monoSelectors(settingsCss(sheet)).sort(),
        `mono outside the allowlist in ${sheet}`,
      ).toEqual([...(allowlist.get(sheet) ?? [])].sort());
    }
  });

  it("keeps the panel explanation sentences", () => {
    const pages = SETTINGS_MENU.flatMap((group) => group.pages);
    const profiles = pages.find((page) => page.id === "profiles")?.intro ?? "";
    const providers = pages.find((page) => page.id === "providers")?.intro ?? "";
    expect(profiles).toContain("The order here is the order agents read");
    expect(providers).toContain("An executable is not a login");
  });
});
