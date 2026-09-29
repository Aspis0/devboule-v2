// The design slice's 12px type floor (SPEC-tokens: "Nothing below 12").
// The walk itself lives in typeFloor.ts; this file only names the sheets it
// walks, one by one — a sheet missing from the list is a sheet not
// walked. A sibling slice walks its own sheets in its own file.
// @vitest-environment node
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { findBelowTypeFloor } from "./typeFloor";

const rootDir = resolve(import.meta.dirname, "../..");
const SHEET_PATHS = [
  "src/styles/tokens.css",
  "src/features/design/design.css",
  "src/features/design/designSession.css",
  "src/features/design/artifactPreview.css",
];

// Sheets land in subfolders as the slice splits: a top-level read lets a
// nested sheet dodge the walk.
function designSheetsOnDisk(): string[] {
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
  walk(resolve(rootDir, "src/features/design"), "");
  return found.sort();
}

function listedDesignSheets(): string[] {
  return SHEET_PATHS.filter((path) => path.startsWith("src/features/design/"))
    .map((path) => path.slice("src/features/design/".length))
    .sort();
}

describe("the design slice's 12px type floor", () => {
  it("walks exactly the three design sheets plus tokens, by name", () => {
    expect(SHEET_PATHS.map((path) => basename(path))).toEqual([
      "tokens.css",
      "design.css",
      "designSession.css",
      "artifactPreview.css",
    ]);
  });

  it("lists every design sheet, so a new file cannot dodge the walk", () => {
    const onDisk = designSheetsOnDisk();
    const listed = listedDesignSheets();
    expect(
      onDisk.filter((name) => !listed.includes(name)),
      `design sheets on disk but not listed: ${onDisk.filter((name) => !listed.includes(name)).join(", ")}`,
    ).toEqual([]);
    expect(
      listed.filter((name) => !onDisk.includes(name)),
      `design sheets listed but missing on disk: ${listed.filter((name) => !onDisk.includes(name)).join(", ")}`,
    ).toEqual([]);
  });

  it("declares no text size below 12px", () => {
    // Read inside the test: a listed sheet deleted from disk fails here
    // with a named error instead of an import-time ENOENT that kills the file.
    const sheets = SHEET_PATHS.map((path) => {
      if (!existsSync(resolve(rootDir, path)))
        throw new Error(`listed sheet missing on disk: ${path}`);
      return readFileSync(resolve(rootDir, path), "utf8");
    });
    // The type tokens are declared once, in :root, so today both runs judge the
    // same sizes; the dark run catches a type token a theme block ever overrides.
    const findings = [
      ...findBelowTypeFloor(sheets, "light"),
      ...findBelowTypeFloor(sheets, "dark"),
    ];
    expect(findings).toEqual([]);
  });
});
