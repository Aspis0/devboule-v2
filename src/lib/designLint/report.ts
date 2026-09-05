// Report entry: run the design lint over Devboule's own source.
//
//   node src/lib/designLint/report.ts
//
// Walks src/**\/*.{tsx,css}, prints findings grouped by file, and ends
// with a total. Read-only; never edits anything. Test files are skipped:
// they deliberately contain examples of every rule.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { DesignLintFinding, DesignTokenMap } from "./designLint";

// Runtime import uses the explicit .ts extension because this file is
// executed directly by Node's type-stripping loader, whose ESM resolver
// demands real specifiers. A static ".ts" specifier would trip tsc's
// Bundler resolution (TS5097, allowImportingTsExtensions is off), so the
// value import stays dynamic while the type import above is erased.
const { extractCustomPropertyTokens, lintSource } = (await import(
  new URL("./designLint.ts", import.meta.url).href
)) as {
  extractCustomPropertyTokens: (source: string) => DesignTokenMap;
  lintSource: (
    source: string,
    filename: string,
    knownTokens?: DesignTokenMap,
  ) => DesignLintFinding[];
};

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC_ROOT = join(HERE, "..", "..", "..");

function walk(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      walk(full, out);
    } else if (/\.(tsx|css)$/i.test(entry.name) && !/\.test\./.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

function run(): number {
  const files = walk(join(SRC_ROOT, "src")).sort();
  const globalCssRel = "src/styles/global.css";
  const tokens = extractCustomPropertyTokens(readFileSync(join(SRC_ROOT, globalCssRel), "utf8"));

  let total = 0;
  for (const file of files) {
    const rel = relative(SRC_ROOT, file).split(sep).join("/");
    const findings: DesignLintFinding[] = lintSource(
      readFileSync(file, "utf8"),
      rel,
      rel === globalCssRel ? undefined : tokens,
    );
    if (findings.length === 0) continue;
    console.log(`\n${rel}`);
    for (const f of findings) {
      console.log(
        `  ${String(f.line).padStart(4)}:${String(f.column ?? 1).padStart(3)}  ${f.severity.padEnd(7)} ${f.rule.padEnd(18)} ${f.message}`,
      );
      total++;
    }
  }
  console.log(`\n${total} finding${total === 1 ? "" : "s"} in ${files.length} files.`);
  return total;
}

const invokedDirectly =
  process.argv[1] !== undefined && pathToFileURL(process.argv[1]).href === import.meta.url;

if (invokedDirectly) {
  run();
}
