import { readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join, posix, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// This is a fixture, not live repository data. The real city will arrive from
// the host's CKG over the postMessage bridge when that route exists.

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const PROJECT_ROOT = resolve(SCRIPT_DIR, "../../..");
const DEFAULT_OUTPUT = resolve(SCRIPT_DIR, "../src/fixture-city.json");
const TYPESCRIPT_EXTENSIONS = [".ts", ".tsx", ".js", ".jsx", ".mjs", ".json"];

export function countLines(source) {
  if (source.length === 0) return 0;
  const lineBreaks = source.match(/\r\n|\r|\n/g)?.length ?? 0;
  return lineBreaks + (source.endsWith("\n") || source.endsWith("\r") ? 0 : 1);
}

export function extractCityFromSources(sources) {
  const normalizedSources = sources
    .map((source) => ({ path: normalizePath(source.path), source: source.source }))
    .sort((left, right) => left.path.localeCompare(right.path));
  const knownFiles = new Set(normalizedSources.map((source) => source.path));
  const files = normalizedSources.map(({ path, source }) => ({
    id: path,
    path,
    lines: countLines(source),
    district: path.split("/")[0],
  }));
  const edgeWeights = new Map();

  for (const source of normalizedSources) {
    for (const specifier of extractImportSpecifiers(source.path, source.source)) {
      const target = resolveSpecifier(source.path, specifier, knownFiles);
      if (target === null || target === source.path) continue;
      const key = `${source.path}\u0000${target}`;
      edgeWeights.set(key, (edgeWeights.get(key) ?? 0) + 1);
    }
  }

  const imports = [...edgeWeights.entries()]
    .map(([key, weight]) => {
      const [from, to] = key.split("\u0000");
      return { from, to, weight };
    })
    .sort((left, right) => left.from.localeCompare(right.from) || left.to.localeCompare(right.to));

  return { files, imports };
}

function extractImportSpecifiers(filePath, source) {
  if (/\.(?:[cm]?js|[jt]sx?)$/i.test(filePath)) {
    const specifiers = [];
    const importFrom = /\bimport\s+(?:type\s+)?[^;\n]*?\s+from\s+["']([^"']+)["']/g;
    for (const match of source.matchAll(importFrom))
      specifiers.push({ kind: "typescript", value: match[1] });
    return specifiers;
  }
  if (filePath.endsWith(".rs")) {
    const specifiers = [];
    const rustUse = /\buse\s+crate::([A-Za-z0-9_]+(?:::[A-Za-z0-9_]+)*)/g;
    for (const match of source.matchAll(rustUse))
      specifiers.push({ kind: "rust", value: match[1] });
    return specifiers;
  }
  return [];
}

function resolveSpecifier(sourcePath, specifier, knownFiles) {
  if (specifier.kind === "typescript") {
    if (!specifier.value.startsWith(".")) return null;
    const base = posix.normalize(posix.join(posix.dirname(sourcePath), specifier.value));
    const candidates = [base, ...TYPESCRIPT_EXTENSIONS.map((extension) => `${base}${extension}`)];
    for (const extension of TYPESCRIPT_EXTENSIONS) candidates.push(`${base}/index${extension}`);
    return candidates.find((candidate) => knownFiles.has(candidate)) ?? null;
  }

  const segments = sourcePath.split("/");
  const srcIndex = segments.lastIndexOf("src");
  if (srcIndex < 0) return null;
  const sourceRoot = segments.slice(0, srcIndex + 1).join("/");
  const moduleSegments = specifier.value.split("::");
  for (let length = moduleSegments.length; length > 0; length -= 1) {
    const modulePath = `${sourceRoot}/${moduleSegments.slice(0, length).join("/")}`;
    for (const candidate of [`${modulePath}.rs`, `${modulePath}/mod.rs`]) {
      if (knownFiles.has(candidate)) return candidate;
    }
  }
  return null;
}

async function collectFiles(root, output = []) {
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const absolute = join(root, entry.name);
    if (entry.isDirectory()) await collectFiles(absolute, output);
    else if (entry.isFile()) output.push(absolute);
  }
  return output;
}

async function collectSources() {
  const roots = [join(PROJECT_ROOT, "src"), join(PROJECT_ROOT, "src-tauri", "src")];
  const crateRoot = join(PROJECT_ROOT, "crates");
  for (const entry of await readdir(crateRoot, { withFileTypes: true })) {
    if (entry.isDirectory()) roots.push(join(crateRoot, entry.name, "src"));
  }
  const paths = [];
  for (const root of roots) {
    try {
      await collectFiles(root, paths);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
  return Promise.all(
    paths.map(async (path) => ({
      path: relative(PROJECT_ROOT, path),
      source: await readFile(path, "utf8"),
    })),
  );
}

function normalizePath(path) {
  return path.replaceAll("\\", "/").replace(/^\.\//, "");
}

function outputPathFromArguments(argv) {
  const outputFlag = argv.indexOf("--out");
  if (outputFlag < 0) return DEFAULT_OUTPUT;
  const value = argv[outputFlag + 1];
  if (!value) throw new Error("--out needs a path");
  return resolve(SCRIPT_DIR, "..", value);
}

async function main() {
  const outputPath = outputPathFromArguments(process.argv.slice(2));
  const city = extractCityFromSources(await collectSources());
  await writeFile(outputPath, `${JSON.stringify(city, null, 2)}\n`, "utf8");
  process.stderr.write(
    `wrote fixture city ${outputPath} (${city.files.length} files, ${city.imports.length} directed roads)\n`,
  );
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (import.meta.url === invokedPath) await main();

export { extractImportSpecifiers, resolveSpecifier };
