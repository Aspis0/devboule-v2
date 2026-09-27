// The inventory the mandate asked for, discovered from the source instead of
// written down: every production file that renders a dialog must register it
// with the shared modal hook, and every one that renders a menu with the
// shared menu hook. A new surface that forgets to register fails here —
// the scan is the only thing that can see a surface nobody remembered to
// add to a list.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const rootDir = resolve(import.meta.dirname, "../..");

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

function sourceOf(file: string): string {
  return readFileSync(file, "utf8");
}

function filesRendering(pattern: RegExp): string[] {
  return productionSources.filter((file) => pattern.test(sourceOf(file)));
}

describe("every dialog registers with the shared modal hook", () => {
  // Rendered roles only: a query string (`closest('[role="dialog"]')`) or a
  // comment that names the attribute is not a surface the shell must know.
  const dialogs = filesRendering(/(?<!\[)role="dialog"|aria-modal="true"/);

  it("finds the dialogs to check", () => {
    expect(dialogs.length).toBeGreaterThan(0);
  });

  it.each(dialogs.map((file) => [relative(rootDir, file), file] as const))(
    "%s registers its dialog",
    (_, file) => {
      expect(sourceOf(file)).toContain("useModalOpen(");
    },
  );
});

describe("every menu registers with the shared menu hook", () => {
  const menus = filesRendering(
    /(?<!\[)role="menu"|(?<!\[)role="menuitem"|(?<!\[)role="listbox"|role=\{[^}]*"listbox"/,
  );

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
