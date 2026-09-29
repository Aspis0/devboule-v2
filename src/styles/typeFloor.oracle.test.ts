// The oracle slice's 12px type floor (SPEC-tokens: "Nothing below 12").
// The walk itself lives in typeFloor.ts; this file names the sheets the
// slice owns, one by one — a sheet missing from the list is a sheet not
// walked — and the folder scan below fails for any sheet the list has not
// named. The themes are asserted separately so a red run names each
// declaration once instead of twice.
// @vitest-environment node
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { findBelowTypeFloor } from "./typeFloor";

const rootDir = resolve(import.meta.dirname, "../..");
const SHEET_PATHS = ["src/styles/tokens.css", "src/features/oracle/oracle.css"];

// Sheets land in subfolders as the slice splits: a top-level read lets a
// nested sheet dodge the walk.
function oracleSheetsOnDisk(): string[] {
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
  walk(resolve(rootDir, "src/features/oracle"), "");
  return found.sort();
}

function listedOracleSheets(): string[] {
  return SHEET_PATHS.filter((path) => path.startsWith("src/features/oracle/"))
    .map((path) => path.slice("src/features/oracle/".length))
    .sort();
}

// Read inside the test: a listed sheet deleted from disk fails here with a
// named error instead of an import-time ENOENT that kills the file.
function sheets(): string[] {
  return SHEET_PATHS.map((path) => {
    if (!existsSync(resolve(rootDir, path)))
      throw new Error(`listed sheet missing on disk: ${path}`);
    return readFileSync(resolve(rootDir, path), "utf8");
  });
}

describe("the oracle slice's 12px type floor", () => {
  it("walks exactly tokens.css and oracle.css, by name", () => {
    expect(SHEET_PATHS.map((path) => basename(path))).toEqual(["tokens.css", "oracle.css"]);
  });

  it("lists every sheet under src/features/oracle, so a new sheet cannot dodge the walk", () => {
    const onDisk = new Set(oracleSheetsOnDisk());
    const listed = new Set(listedOracleSheets());
    const unlisted = [...onDisk].filter((name) => !listed.has(name));
    const missing = [...listed].filter((name) => !onDisk.has(name));
    expect(unlisted, `oracle sheets on disk but not listed: ${unlisted.join(", ")}`).toEqual([]);
    expect(missing, `oracle sheets listed but missing on disk: ${missing.join(", ")}`).toEqual([]);
  });

  it("declares no text size below 12px in the light theme", () => {
    expect(findBelowTypeFloor(sheets(), "light")).toEqual([]);
  });

  it("declares no text size below 12px in the dark theme", () => {
    expect(findBelowTypeFloor(sheets(), "dark")).toEqual([]);
  });
});
