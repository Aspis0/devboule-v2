// The settings slice's 12px type floor (SPEC-tokens: "Nothing below 12").
// The walk itself lives in typeFloor.ts; this file only names the sheets the
// slice owns, one by one — a sheet missing from the list is a sheet not
// walked. A sibling slice walks its own sheets in its own file.
// @vitest-environment node
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { findBelowTypeFloor } from "./typeFloor";

const rootDir = resolve(import.meta.dirname, "../..");
const SHEET_PATHS = [
  "src/styles/tokens.css",
  "src/features/settings/settings.css",
  "src/features/settings/profiles.css",
  "src/features/settings/devices.css",
  "src/features/settings/providers.css",
  "src/features/settings/diagnostics.css",
  "src/features/settings/general.css",
  "src/features/settings/projects.css",
  "src/features/marketplace/marketplace.css",
  "src/styles/global.css",
  "src/features/history/history.css",
  "src/features/polis/polis.css",
  "src/app/errorBoundaries.css",
];

// Sheets land in subfolders as the slice splits (devices/, panels/, ...):
// a top-level read lets a nested sheet dodge the walk.
function settingsSheetsOnDisk(): string[] {
  const found: string[] = [];
  const walk = (dir: string, prefix: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        walk(resolve(dir, entry.name), prefix === "" ? entry.name : `${prefix}/${entry.name}`);
      } else if (entry.name.endsWith(".css")) {
        found.push(prefix === "" ? entry.name : `${prefix}/${entry.name}`);
      }
    }
  };
  walk(resolve(rootDir, "src/features/settings"), "");
  return found.sort();
}

function listedSettingsSheets(): string[] {
  return SHEET_PATHS.filter((path) => path.startsWith("src/features/settings/"))
    .map((path) => path.slice("src/features/settings/".length))
    .sort();
}

describe("the settings slice's 12px type floor", () => {
  it("walks exactly the thirteen sheets the slice owns, by name", () => {
    expect(SHEET_PATHS.map((path) => basename(path))).toEqual([
      "tokens.css",
      "settings.css",
      "profiles.css",
      "devices.css",
      "providers.css",
      "diagnostics.css",
      "general.css",
      "projects.css",
      "marketplace.css",
      "global.css",
      "history.css",
      "polis.css",
      "errorBoundaries.css",
    ]);
  });

  it("lists every settings sheet, so a new file cannot dodge the walk", () => {
    const onDisk = settingsSheetsOnDisk();
    const listed = listedSettingsSheets();
    expect(
      onDisk.filter((name) => !listed.includes(name)),
      `settings sheets on disk but not listed: ${onDisk.filter((name) => !listed.includes(name)).join(", ")}`,
    ).toEqual([]);
    expect(
      listed.filter((name) => !onDisk.includes(name)),
      `settings sheets listed but missing on disk: ${listed.filter((name) => !onDisk.includes(name)).join(", ")}`,
    ).toEqual([]);
  });

  it("declares no text size below 12px, in either theme", () => {
    // Read inside the test: a listed sheet deleted from disk fails here
    // with a named error instead of an import-time ENOENT that kills the file.
    const sheets = SHEET_PATHS.map((path) => {
      if (!existsSync(resolve(rootDir, path)))
        throw new Error(`listed sheet missing on disk: ${path}`);
      return readFileSync(resolve(rootDir, path), "utf8");
    });
    const findings = [
      ...findBelowTypeFloor(sheets, "light"),
      ...findBelowTypeFloor(sheets, "dark"),
    ];
    expect(findings).toEqual([]);
  });
});
