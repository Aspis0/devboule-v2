// The inventory the mandate asked for, discovered from the source instead of
// written down: every production file that renders a dialog registers it with
// a registry the shell honours, and every one that renders a menu with the
// shared menu hook. A new surface that forgets to register fails here —
// the scan is the only thing that can see a surface nobody remembered to
// add to a list.
//
// Scope, stated plainly, because the limits are the point:
// - Comments are stripped before matching: a comment that names the
//   attribute, or a query string (`closest('[role="dialog"]')`), is not a
//   render — the patterns carry a lookbehind for the query form.
// - A role built from a named constant (`const R = "dialog"; <div role={R}>`)
//   is not textually resolvable and is out of scope; the contract is that
//   the file that renders a surface registers it, and the rendered walk in
//   modals-over-crescent.dialogs.test.tsx is the per-surface backstop.
// - The check is per FILE, not per surface: a file holding several
//   surfaces is checked as a whole. The rendered walk asserts the right
//   registry per surface at runtime.
// - A dialog registers with EITHER registry — a modal (an in-flight answer
//   to protect) or a menu (the context popover is informational). Which one
//   is right is a behaviour question the walk answers, not a grep.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const rootDir = resolve(import.meta.dirname, "../..");

/** Block comments, then line comments outside strings. */
function stripComments(source: string): string {
  const withoutBlock = source.replace(/\/\*[\s\S]*?\*\//g, "");
  let result = "";
  let quote: string | null = null;
  for (let i = 0; i < withoutBlock.length; i += 1) {
    const ch = withoutBlock[i]!;
    if (quote !== null) {
      result += ch;
      if (ch === quote && withoutBlock[i - 1] !== "\\") quote = null;
      continue;
    }
    if (ch === "/" && withoutBlock[i + 1] === "/") {
      while (i < withoutBlock.length && withoutBlock[i] !== "\n") i += 1;
      result += "\n";
      continue;
    }
    if (ch === '"' || ch === "'" || ch === "`") {
      quote = ch;
    }
    result += ch;
  }
  return result;
}

const DIALOG_RENDER =
  /(?<!\[)role="dialog"|(?<!\[)role='dialog'|(?<!\[)role="alertdialog"|(?<!\[)role='alertdialog'|role=\{[^}]*"dialog"|aria-modal=\{|aria-modal="true"|aria-modal='true'|role:\s*"dialog"/;
const MENU_RENDER =
  /(?<!\[)role="menu"|(?<!\[)role='menu'|(?<!\[)role="menuitem"|(?<!\[)role='menuitem'|(?<!\[)role="listbox"|(?<!\[)role='listbox'|role=\{[^}]*"(menu|menuitem|listbox)"/;

/** The rendered surfaces one source declares, comments and queries removed. */
export function renderingsIn(source: string, pattern: RegExp): string[] {
  return stripComments(source)
    .split("\n")
    .filter((line) => pattern.test(line));
}

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.tsx?$/.test(entry) ? [path] : [];
  });
}

const productionSources = sourceFiles(join(rootDir, "src")).filter(
  (file) => !/\.(test|spec)\.[jt]sx?$/.test(file) && !file.includes("__tests__"),
);

function filesRendering(pattern: RegExp): string[] {
  return productionSources.filter(
    (file) => renderingsIn(readFileSync(file, "utf8"), pattern).length > 0,
  );
}

function sourceOf(file: string): string {
  return readFileSync(file, "utf8");
}

describe("every dialog registers with a registry the shell honours", () => {
  const dialogs = filesRendering(DIALOG_RENDER);

  it("finds the dialogs to check", () => {
    expect(dialogs.length).toBeGreaterThan(0);
  });

  it.each(dialogs.map((file) => [relative(rootDir, file), file] as const))(
    "%s registers its dialog",
    (_, file) => {
      expect(sourceOf(file)).toMatch(/useModalOpen\(|useMenuOpen\(/);
    },
  );
});

describe("every menu registers with the shared menu hook", () => {
  const menus = filesRendering(MENU_RENDER);

  it("finds the menus to check", () => {
    expect(menus.length).toBeGreaterThan(0);
  });

  it.each(menus.map((file) => [relative(rootDir, file), file] as const))(
    "%s registers its menu",
    (_, file) => {
      expect(sourceOf(file)).toContain("useMenuOpen(");
    },
  );
});

describe("the scan's own rules", () => {
  it("a comment that names the attribute is not a surface", () => {
    expect(renderingsIn('// the card is role="dialog" per ARIA\n<div />', DIALOG_RENDER)).toEqual(
      [],
    );
    expect(renderingsIn('/* role="dialog" */\n<div />', DIALOG_RENDER)).toEqual([]);
  });

  it("a query string is not a surface", () => {
    expect(renderingsIn("el.closest('[role=\"dialog\"]')", DIALOG_RENDER)).toEqual([]);
  });

  it("a ternary role is a dialog", () => {
    expect(renderingsIn('<div role={open ? "dialog" : "group"} />', DIALOG_RENDER)).toHaveLength(1);
  });

  it("a computed aria-modal is a dialog", () => {
    expect(renderingsIn("<div aria-modal={true} />", DIALOG_RENDER)).toHaveLength(1);
  });

  it("a single-quoted role is a dialog", () => {
    expect(renderingsIn("<div role='dialog' />", DIALOG_RENDER)).toHaveLength(1);
  });

  it("an alertdialog role is a dialog", () => {
    expect(renderingsIn('<div role="alertdialog" />', DIALOG_RENDER)).toHaveLength(1);
    expect(renderingsIn("<div role='alertdialog' />", DIALOG_RENDER)).toHaveLength(1);
  });

  it("an alertdialog query string is not a surface", () => {
    expect(renderingsIn("el.closest('[role=\"alertdialog\"]')", DIALOG_RENDER)).toEqual([]);
  });

  it("a menu role is not a dialog", () => {
    expect(renderingsIn('<div role="menu" />', DIALOG_RENDER)).toEqual([]);
  });

  it("a createElement role is a dialog", () => {
    expect(renderingsIn('createElement("div", { role: "dialog" })', DIALOG_RENDER)).toHaveLength(1);
  });

  it("a comment inside a string is not stripped away from the render", () => {
    expect(
      renderingsIn('const url = "http://x";\n<div role="dialog" />', DIALOG_RENDER),
    ).toHaveLength(1);
  });

  it("a ternary menu is a menu", () => {
    expect(renderingsIn('<div role={x ? "menuitem" : "none"} />', MENU_RENDER)).toHaveLength(1);
  });

  it("a role from a named constant is out of scope — stated above, not silently missed", () => {
    expect(renderingsIn('const R = "dialog";\n<div role={R} />', DIALOG_RENDER)).toEqual([]);
  });
});
