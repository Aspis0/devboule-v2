import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { SETTINGS_MENU } from "./settingsMenu";

// Static contracts on the settings stylesheets and catalogue: no rendering,
// no root, no afterEach. Each guard parses the declaration it pins, so a
// mutation run fails on the assertion below rather than on a hook.

function ruleBody(css: string, selector: string): string | null {
  const start = css.indexOf(selector);
  if (start === -1) return null;
  const open = css.indexOf("{", start);
  const close = open === -1 ? -1 : css.indexOf("}", open);
  if (open === -1 || close === -1) return null;
  return css.slice(open + 1, close);
}

/** The value of the `outline` declaration in the selector's rule, if any. */
function outlineValue(css: string, selector: string): string | null {
  const body = ruleBody(css, selector);
  if (body === null) return null;
  for (const declaration of body.split(";")) {
    const colon = declaration.indexOf(":");
    if (colon === -1) continue;
    if (declaration.slice(0, colon).trim() === "outline") {
      return declaration.slice(colon + 1).trim();
    }
  }
  return null;
}

function settingsCss(name: string): string {
  return readFileSync(resolve(import.meta.dirname, name), "utf8");
}

describe("Settings static contracts", () => {
  it("rings keyboard focus on the menu rows with a drawn outline", () => {
    const outline = outlineValue(settingsCss("settings.css"), ".settings-menu-row:focus-visible");
    expect(outline).not.toBeNull();
    expect(outline).not.toMatch(/^(none|0|transparent)$/i);
    expect(outline).toContain("solid");
  });

  it("keeps its screen-reader utility inside the settings styles", () => {
    const srOnly = /\.sr-only\s*\{([^}]*)\}/.exec(settingsCss("settings.css"))?.[1] ?? "";
    expect(srOnly).toContain("position: absolute");
    expect(srOnly).toContain("overflow: hidden");
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
