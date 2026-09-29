// Every stylesheet the app ships, as text: the sheets the style walks police.
// Paths are repo-relative with forward slashes so a finding names the file
// the same way on every platform.

import { readdirSync, readFileSync } from "node:fs";
import { join, resolve, sep } from "node:path";

export interface SrcSheet {
  path: string;
  css: string;
}

const SRC_ROOT = resolve(import.meta.dirname, "..");

export function collectSrcSheets(): SrcSheet[] {
  return readdirSync(SRC_ROOT, { recursive: true, encoding: "utf8" })
    .map((entry) => entry.split(sep).join("/"))
    .filter((entry) => entry.endsWith(".css"))
    .sort()
    .map((entry) => ({
      path: `src/${entry}`,
      css: readFileSync(join(SRC_ROOT, entry), "utf8"),
    }));
}
