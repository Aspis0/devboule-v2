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
    for (const css of [profiles, diagnostics]) {
      expect(css).not.toContain(".settings-subheading");
    }
    const rule = /\.settings-subheading\s*\{([^}]*)\}/.exec(shell)?.[1] ?? "";
    expect(rule).toContain("font-size: 12px");
    expect(rule).toContain("font-weight: 500");
  });

  it("keeps the panel explanation sentences", () => {
    const pages = SETTINGS_MENU.flatMap((group) => group.pages);
    const profiles = pages.find((page) => page.id === "profiles")?.intro ?? "";
    const providers = pages.find((page) => page.id === "providers")?.intro ?? "";
    expect(profiles).toContain("The order here is the order agents read");
    expect(providers).toContain("An executable is not a login");
  });
});
