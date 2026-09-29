// The settings slice's 12px type floor (SPEC-tokens: "Nothing below 12").
// The walk itself lives in typeFloor.ts; this file only names the sheets the
// slice owns, one by one — a sheet missing from the list is a sheet not
// walked. A sibling slice walks its own sheets in its own file.
// @vitest-environment node
import { readFileSync } from "node:fs";
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
const SHEETS = SHEET_PATHS.map((path) => readFileSync(resolve(rootDir, path), "utf8"));

describe("the settings slice's 12px type floor", () => {
  it("walks exactly the twelve sheets the slice owns, by name", () => {
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

  it("declares no text size below 12px, in either theme", () => {
    const findings = [
      ...findBelowTypeFloor(SHEETS, "light"),
      ...findBelowTypeFloor(SHEETS, "dark"),
    ];
    expect(findings).toEqual([]);
  });
});
