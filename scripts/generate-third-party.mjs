import { execFile } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const thirdPartyPath = resolve(repoRoot, "THIRD_PARTY.md");
const NPM_REGISTRY = "https://registry.npmjs.org";
const NPM_REQUEST_TIMEOUT_MS = 15_000;
const NPM_CONCURRENCY = 8;

// `pnpm licenses list` is an installed-tree report: it can omit lockfile-only
// platform/dev records and groups multiple versions of a name in `versions`.
// The lockfile's `packages` keys therefore define the npm record set here;
// package.json metadata is read for that exact name and version locally when
// present, otherwise from the npm registry.

const markers = {
  rust: {
    begin: "<!-- BEGIN GENERATED THIRD-PARTY RUST -->",
    end: "<!-- END GENERATED THIRD-PARTY RUST -->",
    heading: /^### Rust registry packages(?: \([^)]*\))?$/,
  },
  npm: {
    begin: "<!-- BEGIN GENERATED THIRD-PARTY NPM -->",
    end: "<!-- END GENERATED THIRD-PARTY NPM -->",
    heading: /^### npm packages(?: \([^)]*\))?$/,
  },
};

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

function compareText(left, right) {
  if (left < right) return -1;
  if (left > right) return 1;
  return 0;
}

function compareRecords(left, right) {
  return (
    compareText(left.name, right.name) ||
    compareText(left.version, right.version) ||
    compareText(left.kind, right.kind) ||
    compareText(left.license, right.license)
  );
}

function requireLicense(value, packageLabel) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`Package ${packageLabel} has no declared license.`);
  }
  if (/[\r\n]/u.test(value)) {
    throw new Error(`Package ${packageLabel} has a multi-line license field.`);
  }
  return value;
}

function escapeTableCell(value) {
  return String(value)
    .replaceAll("|", "\\|")
    .replace(/[\r\n]/gu, " ");
}

function renderTable(records) {
  return [
    "| Name | Version | Kind | Licence |",
    "| --- | --- | --- | --- |",
    ...records.map(
      (record) =>
        `| ${escapeTableCell(record.name)} | ${escapeTableCell(record.version)} | ${escapeTableCell(record.kind)} | ${escapeTableCell(record.license)} |`,
    ),
  ];
}

function renderGeneratedSection(kind, records, newline) {
  const config = markers[kind];
  const lockfile = kind === "rust" ? "Cargo.lock" : "pnpm-lock.yaml";
  const label = kind === "rust" ? "Rust registry packages" : "npm packages";
  return [
    config.begin,
    `### ${label} (${lockfile}; ${records.length} records)`,
    "",
    ...renderTable(records),
    "",
    config.end,
  ].join(newline);
}

function splitLines(text) {
  return text.split(/\r?\n/u);
}

function markdownLineVisibility(lines) {
  const visible = new Array(lines.length).fill(true);
  let fence = null;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    visible[index] = fence === null;

    if (fence) {
      const closing = new RegExp(`^\\s{0,3}${fence.char}{${fence.length},}\\s*$`, "u");
      if (closing.test(line)) fence = null;
      continue;
    }

    const opening = line.match(/^\s{0,3}(`{3,}|~{3,})/u);
    if (opening) {
      fence = { char: opening[1][0], length: opening[1].length };
    }
  }

  return visible;
}

function newlineFor(text) {
  return text.includes("\r\n") ? "\r\n" : "\n";
}

function headingLevel(line) {
  const match = line.match(/^(#{1,6})[ \t]+/u);
  return match ? match[1].length : null;
}

function findAll(lines, predicate) {
  const indexes = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (predicate(lines[index], index)) indexes.push(index);
  }
  return indexes;
}

function replaceOneSection(lines, kind, replacementLines, migrationMessages) {
  const config = markers[kind];
  const visible = markdownLineVisibility(lines);
  const beginIndexes = findAll(
    lines,
    (line, index) => visible[index] && line.trim() === config.begin,
  );
  const endIndexes = findAll(lines, (line, index) => visible[index] && line.trim() === config.end);

  if (beginIndexes.length !== endIndexes.length || beginIndexes.length > 1) {
    throw new Error(
      `${kind} generated markers are incomplete or duplicated; expected exactly one BEGIN/END pair.`,
    );
  }

  if (beginIndexes.length === 1) {
    const begin = beginIndexes[0];
    const end = endIndexes[0];
    if (begin >= end) {
      throw new Error(`${kind} generated markers are out of order.`);
    }
    if (
      !lines
        .slice(begin, end + 1)
        .some((line, offset) => visible[begin + offset] && config.heading.test(line.trim()))
    ) {
      throw new Error(`${kind} generated markers do not contain the expected section heading.`);
    }
    return [...lines.slice(0, begin), ...replacementLines, ...lines.slice(end + 1)];
  }

  const headingIndexes = findAll(
    lines,
    (line, index) => visible[index] && config.heading.test(line.trim()),
  );
  if (headingIndexes.length !== 1) {
    throw new Error(
      `${kind} section heading is missing or ambiguous; cannot insert generated markers safely.`,
    );
  }

  const headingIndex = headingIndexes[0];
  const nextHeadingIndex = lines.findIndex(
    (line, index) =>
      index > headingIndex &&
      visible[index] &&
      (headingLevel(line) ?? Number.POSITIVE_INFINITY) <= 3,
  );
  if (nextHeadingIndex === -1) {
    throw new Error(
      `${kind} section has no following level-1/2/3 heading; refusing to guess where human prose ends.`,
    );
  }

  const discardedHumanLines = lines
    .slice(headingIndex + 1, nextHeadingIndex)
    .flatMap((line, offset) => {
      if (line.trim() === "" || /^\s*\|.*\|\s*$/u.test(line)) return [];
      return [{ line: headingIndex + offset + 2, text: line.trim() }];
    });
  if (discardedHumanLines.length > 0) {
    const first = discardedHumanLines[0];
    throw new Error(
      `${kind} migration would discard human content at line ${first.line}: ${JSON.stringify(first.text)}. ` +
        "Remove the note or add generated markers manually before retrying.",
    );
  }

  migrationMessages.push(
    `MIGRATE: will wrap the table-only ${kind} section lines ${headingIndex + 1}-${nextHeadingIndex} with generated markers.`,
  );
  return [...lines.slice(0, headingIndex), ...replacementLines, ...lines.slice(nextHeadingIndex)];
}

export function replaceInventorySections(markdown, sections) {
  const newline = newlineFor(markdown);
  let lines = splitLines(markdown);
  const migrationMessages = [];

  for (const kind of ["rust", "npm"]) {
    const replacement = renderGeneratedSection(kind, sections[kind], newline);
    lines = replaceOneSection(lines, kind, splitLines(replacement), migrationMessages);
  }

  return { markdown: lines.join(newline), migrationMessages };
}

function humanProse(markdown) {
  const lines = splitLines(markdown);
  const visible = markdownLineVisibility(lines);
  const humanLines = [];
  let generated = false;

  for (let index = 0; index < lines.length; index += 1) {
    if (!visible[index]) continue;
    const line = lines[index].trim();
    if (Object.values(markers).some((config) => line === config.begin)) {
      generated = true;
      continue;
    }
    if (Object.values(markers).some((config) => line === config.end)) {
      generated = false;
      continue;
    }
    if (!generated) humanLines.push(lines[index]);
  }

  return humanLines.join("\n");
}

function validateCountProse(markdown, { cargoPackageCount, rustCount, npmCount }) {
  const prose = humanProse(markdown);
  const errors = [];

  for (const match of prose.matchAll(/\b(\d+) registry packages\b/gu)) {
    if (Number(match[1]) !== rustCount) {
      errors.push(`registry package count ${match[1]} (expected ${rustCount})`);
    }
  }
  for (const match of prose.matchAll(
    /\b(?:approximately )?(\d+) package records in `?pnpm-lock\.yaml`?/gu,
  )) {
    if (Number(match[1]) !== npmCount) {
      errors.push(`npm package count ${match[1]} (expected ${npmCount})`);
    }
  }
  for (const match of prose.matchAll(/\b(\d+) package records in `?Cargo\.lock`?/gu)) {
    if (Number(match[1]) !== cargoPackageCount) {
      errors.push(`Cargo.lock package count ${match[1]} (expected ${cargoPackageCount})`);
    }
  }
  for (const match of prose.matchAll(/\b(\d+) entries in `?Cargo\.lock`?/gu)) {
    if (Number(match[1]) !== cargoPackageCount) {
      errors.push(`Cargo.lock entry count ${match[1]} (expected ${cargoPackageCount})`);
    }
  }
  for (const match of prose.matchAll(/"(\d+) package records"/gu)) {
    if (Number(match[1]) !== npmCount) {
      errors.push(`quoted npm package count ${match[1]} (expected ${npmCount})`);
    }
  }

  if (errors.length > 0) {
    throw new Error(
      `Human-owned count prose is stale: ${errors.join(", ")}. ` +
        "Update the prose manually; the generator never rewrites it.",
    );
  }
}

export function renderDocument(markdown, { rustRecords, npmRecords, cargoPackageCount }) {
  const rendered = replaceInventorySections(markdown, {
    rust: [...rustRecords].sort(compareRecords),
    npm: [...npmRecords].sort(compareRecords),
  });
  validateCountProse(rendered.markdown, {
    cargoPackageCount,
    rustCount: rustRecords.length,
    npmCount: npmRecords.length,
  });
  return rendered;
}

function normalizeCargoName(name) {
  return typeof name === "string" ? name.replaceAll("_", "-") : null;
}

function tokenizeCargoTarget(target) {
  const tokens = [];
  let index = 0;
  while (index < target.length) {
    const character = target[index];
    if (/\s/u.test(character)) {
      index += 1;
      continue;
    }
    if ("(),=".includes(character)) {
      tokens.push(character);
      index += 1;
      continue;
    }
    if (character === '"' || character === "'") {
      const quote = character;
      let value = "";
      index += 1;
      while (index < target.length && target[index] !== quote) {
        value += target[index];
        index += 1;
      }
      if (target[index] !== quote) {
        throw new Error(`Malformed Cargo target expression ${JSON.stringify(target)}.`);
      }
      tokens.push({ type: "value", value });
      index += 1;
      continue;
    }
    const match = target.slice(index).match(/^[A-Za-z_][A-Za-z0-9_.-]*/u);
    if (!match) {
      throw new Error(`Malformed Cargo target expression ${JSON.stringify(target)}.`);
    }
    tokens.push({ type: "value", value: match[0] });
    index += match[0].length;
  }
  return tokens;
}

function parseCargoTarget(target) {
  const tokens = tokenizeCargoTarget(target);
  let index = 0;

  function parseTerm() {
    const identifier = tokens[index++];
    if (!identifier || identifier.type !== "value") {
      throw new Error(`Malformed Cargo target expression ${JSON.stringify(target)}.`);
    }
    if (tokens[index] === "=") {
      index += 1;
      const value = tokens[index++];
      if (!value || value.type !== "value") {
        throw new Error(`Malformed Cargo target expression ${JSON.stringify(target)}.`);
      }
      return { type: "comparison", key: identifier.value, value: value.value };
    }
    if (tokens[index] !== "(") return { type: "atom", value: identifier.value };

    index += 1;
    const args = [];
    while (tokens[index] !== ")") {
      if (index >= tokens.length) {
        throw new Error(`Malformed Cargo target expression ${JSON.stringify(target)}.`);
      }
      args.push(parseTerm());
      if (tokens[index] === ",") index += 1;
      else if (tokens[index] !== ")") {
        throw new Error(`Malformed Cargo target expression ${JSON.stringify(target)}.`);
      }
    }
    index += 1;
    return { type: "call", name: identifier.value, args };
  }

  const expression = parseTerm();
  if (index !== tokens.length) {
    throw new Error(`Malformed Cargo target expression ${JSON.stringify(target)}.`);
  }
  return expression;
}

function positiveWindowsTarget(node, negated = false) {
  if (node.type === "atom") {
    return !negated && /(?:^|-)windows(?:-|$)/u.test(node.value);
  }
  if (node.type === "comparison") {
    return !negated && node.value === "windows";
  }
  if (node.type === "call") {
    if (node.name === "not") {
      return node.args.length === 1 && positiveWindowsTarget(node.args[0], !negated);
    }
    return node.args.some((argument) => positiveWindowsTarget(argument, negated));
  }
  return false;
}

function targetRequiresWindows(target) {
  if (!target) return false;
  return positiveWindowsTarget(parseCargoTarget(target));
}

function directCargoKinds(metadata) {
  const workspaceMembers = new Set(metadata.workspace_members ?? []);
  const packagesById = new Map((metadata.packages ?? []).map((item) => [item.id, item]));
  const kindsById = new Map();

  for (const node of metadata.resolve?.nodes ?? []) {
    if (!workspaceMembers.has(node.id)) continue;
    const owner = packagesById.get(node.id);
    for (const dependency of node.deps ?? []) {
      const target = packagesById.get(dependency.pkg);
      if (!target) continue;

      const kinds = kindsById.get(target.id) ?? new Set();
      for (const depKind of dependency.dep_kinds ?? []) {
        kinds.add(depKind.kind ?? "normal");
        kinds.add(depKind.target ? "targeted" : "untargeted");
      }

      const normalizedDependencyName = normalizeCargoName(dependency.name);
      const declarations = (owner?.dependencies ?? []).filter((item) =>
        [item.name, item.rename, item.package]
          .map(normalizeCargoName)
          .some((name) => name !== null && name === normalizedDependencyName),
      );
      for (const declaration of declarations) {
        if (declaration.optional) kinds.add("optional");
        kinds.add(declaration.target ? "targeted" : "untargeted");
        if (targetRequiresWindows(declaration.target)) kinds.add("windows");
        if (declaration.kind) kinds.add(declaration.kind);
      }
      for (const depKind of dependency.dep_kinds ?? []) {
        if (targetRequiresWindows(depKind.target)) kinds.add("windows");
      }
      kindsById.set(target.id, kinds);
    }
  }

  return kindsById;
}

function cargoKind(packageInfo, directKinds) {
  const direct = directKinds.get(packageInfo.id);
  if (!direct) {
    return packageInfo.source === null ? "Rust vendored third-party" : "Rust transitive (lockfile)";
  }

  const labels = [];
  const targetOnlyWindows =
    direct.has("targeted") && !direct.has("untargeted") && direct.has("windows");
  if (direct.has("normal") && !targetOnlyWindows) labels.push("runtime");
  if (direct.has("build") && !targetOnlyWindows) labels.push("build");
  if (direct.has("dev") && !targetOnlyWindows) labels.push("test");
  if (direct.has("optional")) labels.push("optional");
  if (direct.has("windows")) labels.push("Windows");
  if (labels.length === 0) labels.push("runtime");
  return `Rust direct ${labels.join(" ")}`;
}

export function collectCargoRecords(metadata) {
  if (!metadata || !Array.isArray(metadata.packages)) {
    throw new Error("cargo metadata did not contain a packages array.");
  }

  const unsupportedSources = metadata.packages.filter(
    (packageInfo) => packageInfo.source !== null && !packageInfo.source?.startsWith("registry+"),
  );
  if (unsupportedSources.length > 0) {
    const first = unsupportedSources[0];
    throw new Error(
      `Cargo package ${first.name}@${first.version} uses unsupported source ${JSON.stringify(first.source)}; git and other non-registry dependencies must be inventoried explicitly before generation can continue.`,
    );
  }

  const workspaceMembers = new Set(metadata.workspace_members ?? []);
  const directKinds = directCargoKinds(metadata);
  const packages = metadata.packages.filter(
    (packageInfo) =>
      packageInfo.source?.startsWith("registry+") ||
      (packageInfo.source === null && !workspaceMembers.has(packageInfo.id)),
  );

  return packages.map((packageInfo) => {
    const label = `Cargo package ${packageInfo.name}@${packageInfo.version}`;
    return {
      name: packageInfo.name,
      version: packageInfo.version,
      kind: cargoKind(packageInfo, directKinds),
      license: requireLicense(packageInfo.license, label),
    };
  });
}

function unquoteYamlScalar(value) {
  if (value.startsWith("'") && value.endsWith("'")) {
    return value.slice(1, -1).replaceAll("''", "'");
  }
  if (value.startsWith('"') && value.endsWith('"')) {
    return JSON.parse(value);
  }
  return value;
}

function packageKeyParts(key) {
  let withoutPeers = key;
  if (key.endsWith(")")) {
    let depth = 0;
    let openingIndex = -1;
    let maximumDepth = 0;
    for (let index = key.length - 1; index >= 0; index -= 1) {
      if (key[index] === ")") {
        depth += 1;
        maximumDepth = Math.max(maximumDepth, depth);
      } else if (key[index] === "(") {
        depth -= 1;
        if (depth === 0) {
          openingIndex = index;
          break;
        }
        if (depth < 0) break;
      }
    }
    if (depth !== 0 || openingIndex < 0) {
      throw new Error(
        `Cannot parse pnpm lockfile package key ${JSON.stringify(key)}: unbalanced peer suffix.`,
      );
    }
    if (maximumDepth > 1) {
      throw new Error(
        `Cannot parse pnpm lockfile package key ${JSON.stringify(key)}: nested peer suffixes are unsupported.`,
      );
    }
    withoutPeers = key.slice(0, openingIndex);
  } else if (key.includes("(")) {
    throw new Error(
      `Cannot parse pnpm lockfile package key ${JSON.stringify(key)}: malformed peer suffix.`,
    );
  }
  const at = withoutPeers.lastIndexOf("@");
  if (at <= 0 || at === withoutPeers.length - 1) {
    throw new Error(`Cannot parse pnpm lockfile package key ${JSON.stringify(key)}.`);
  }
  return {
    name: withoutPeers.slice(0, at),
    version: withoutPeers.slice(at + 1),
  };
}

export function parsePnpmPackageEntries(lockfileText) {
  const lines = splitLines(lockfileText);
  const packagesIndex = lines.findIndex((line) => line.trim() === "packages:");
  const snapshotsIndex = lines.findIndex(
    (line, index) => index > packagesIndex && line.trim() === "snapshots:",
  );
  if (packagesIndex === -1 || snapshotsIndex === -1) {
    throw new Error("pnpm-lock.yaml must contain packages: and snapshots: sections.");
  }

  const entries = [];
  for (let index = packagesIndex + 1; index < snapshotsIndex; index += 1) {
    const match = lines[index].match(/^  (.+):$/u);
    if (!match) continue;

    const key = unquoteYamlScalar(match[1]);
    const parts = packageKeyParts(key);
    const blockEnd = lines.findIndex((line, candidate) => candidate > index && /^  \S/u.test(line));
    const end = blockEnd === -1 || blockEnd > snapshotsIndex ? snapshotsIndex : blockEnd;
    const block = lines.slice(index, end).join("\n");
    entries.push({
      ...parts,
      isPlatform: /^    (?:cpu|os|libc):/mu.test(block),
    });
    index = end - 1;
  }

  if (entries.length === 0) {
    throw new Error("pnpm-lock.yaml packages: section contains no package records.");
  }
  return entries;
}

function readRootPackageJson() {
  try {
    return JSON.parse(readFileSync(resolve(repoRoot, "package.json"), "utf8"));
  } catch (error) {
    throw new Error(`Cannot read package.json: ${errorMessage(error)}`);
  }
}

function npmDirectKinds() {
  const packageJson = readRootPackageJson();
  const kinds = new Map();
  for (const [name] of Object.entries(packageJson.dependencies ?? {})) {
    kinds.set(name, "runtime");
  }
  for (const [name] of Object.entries(packageJson.optionalDependencies ?? {})) {
    kinds.set(name, "optional");
  }
  for (const [name] of Object.entries(packageJson.devDependencies ?? {})) {
    const previous = kinds.get(name);
    kinds.set(name, previous ? `${previous}/build/test` : "build/test");
  }
  return kinds;
}

function npmKind(entry, directKinds) {
  const direct = directKinds.get(entry.name);
  if (direct) return `npm direct ${direct}`;
  return entry.isPlatform ? "npm transitive optional/platform" : "npm transitive";
}

function normalizeNpmLicense(value, packageLabel) {
  if (typeof value === "string") return requireLicense(value, packageLabel);
  if (value && typeof value === "object" && typeof value.type === "string") {
    return requireLicense(value.type, packageLabel);
  }
  if (Array.isArray(value) && value.length > 0 && value.every((item) => item?.type)) {
    return requireLicense(value.map((item) => item.type).join(" OR "), packageLabel);
  }
  throw new Error(`Package ${packageLabel} has no declared license.`);
}

function npmRecordFromMetadata(entry, metadata, directKinds) {
  const label = `npm package ${entry.name}@${entry.version}`;
  if (!metadata || typeof metadata !== "object") {
    throw new Error(`Metadata for ${label} is not an object.`);
  }
  if (metadata.name !== entry.name || metadata.version !== entry.version) {
    throw new Error(
      `Metadata for ${label} resolved to ${metadata.name ?? "<unnamed>"}@${metadata.version ?? "<unversioned>"}.`,
    );
  }
  return {
    name: entry.name,
    version: entry.version,
    kind: npmKind(entry, directKinds),
    license: normalizeNpmLicense(metadata.license ?? metadata.licenses, label),
  };
}

function localPackageJsonPath(name, version) {
  const packageParts = name.split("/");
  const directPath = resolve(repoRoot, "node_modules", ...packageParts, "package.json");
  if (existsSync(directPath)) return directPath;

  const pnpmRoot = resolve(repoRoot, "node_modules", ".pnpm");
  if (!existsSync(pnpmRoot)) return null;
  const encodedName = name.replaceAll("/", "+");
  const prefix = `${encodedName}@${version}`;
  for (const directory of readdirSync(pnpmRoot, { withFileTypes: true })) {
    if (
      directory.isDirectory() &&
      (directory.name === prefix || directory.name.startsWith(`${prefix}(`))
    ) {
      const candidate = resolve(
        pnpmRoot,
        directory.name,
        "node_modules",
        ...packageParts,
        "package.json",
      );
      if (existsSync(candidate)) return candidate;
    }
  }
  return null;
}

export async function fetchNpmMetadata(entry, fetchImpl = globalThis.fetch) {
  const label = `npm package ${entry.name}@${entry.version}`;
  if (typeof fetchImpl !== "function") {
    throw new Error(`${label} cannot be fetched because global fetch is unavailable.`);
  }
  const url = `${NPM_REGISTRY}/${encodeURIComponent(entry.name)}/${encodeURIComponent(entry.version)}`;
  let response;
  try {
    response = await fetchImpl(url, {
      headers: {
        accept: "application/json",
        "user-agent": "devboule-v2-third-party-inventory",
      },
      signal: AbortSignal.timeout(NPM_REQUEST_TIMEOUT_MS),
    });
  } catch (error) {
    throw new Error(`${label} metadata request failed: ${errorMessage(error)}`);
  }
  if (!response.ok) {
    throw new Error(
      `${label} metadata request failed: HTTP ${response.status} ${response.statusText}`,
    );
  }
  let metadata;
  try {
    metadata = await response.json();
  } catch (error) {
    throw new Error(`${label} metadata response was not valid JSON: ${errorMessage(error)}`);
  }
  return metadata;
}

export async function loadNpmMetadata(entry, fetchImpl = globalThis.fetch) {
  const localPath = localPackageJsonPath(entry.name, entry.version);
  if (localPath) {
    let metadata;
    try {
      metadata = JSON.parse(readFileSync(localPath, "utf8"));
    } catch (error) {
      throw new Error(
        `npm package ${entry.name}@${entry.version} local package metadata failed: ${errorMessage(error)}`,
      );
    }
    if (metadata.name === entry.name && metadata.version === entry.version) {
      return metadata;
    }
  }
  return fetchNpmMetadata(entry, fetchImpl);
}

async function mapWithConcurrency(items, worker, concurrency) {
  const results = new Array(items.length);
  let nextIndex = 0;
  async function runWorker() {
    while (true) {
      const index = nextIndex;
      nextIndex += 1;
      if (index >= items.length) return;
      results[index] = await worker(items[index]);
    }
  }
  await Promise.all(Array.from({ length: Math.min(concurrency, items.length) }, () => runWorker()));
  return results;
}

export async function collectNpmRecords(lockfileText, options = {}) {
  const entries = parsePnpmPackageEntries(lockfileText);
  const directKinds = npmDirectKinds();
  const metadataLoader =
    typeof options === "function"
      ? options
      : (options.metadataLoader ?? ((entry) => loadNpmMetadata(entry, options.fetchImpl)));
  const metadata = await mapWithConcurrency(entries, metadataLoader, NPM_CONCURRENCY);
  return entries.map((entry, index) => npmRecordFromMetadata(entry, metadata[index], directKinds));
}

const CARGO_METADATA_MAX_BUFFER = 128 * 1024 * 1024;

export function cargoMetadataFailure(error) {
  if (
    error?.code === "ERR_CHILD_PROCESS_STDIO_MAXBUFFER" ||
    /maxBuffer|max buffer length/i.test(errorMessage(error))
  ) {
    return new Error(
      `cargo metadata exceeded the ${CARGO_METADATA_MAX_BUFFER / 1024 / 1024} MiB stdout limit; increase CARGO_METADATA_MAX_BUFFER if the dependency graph grows further.`,
    );
  }
  return new Error(`cargo metadata failed: ${errorMessage(error)}`);
}

export async function readCargoMetadata(exec = execFileAsync) {
  try {
    const result = await exec(
      "cargo",
      ["metadata", "--format-version", "1", "--all-features", "--locked"],
      { cwd: repoRoot, maxBuffer: CARGO_METADATA_MAX_BUFFER, windowsHide: true },
    );
    return JSON.parse(result.stdout);
  } catch (error) {
    throw cargoMetadataFailure(error);
  }
}

function updateCountsAndRender(markdown, cargoMetadata, rustRecords, npmRecords) {
  return renderDocument(markdown, {
    rustRecords,
    npmRecords,
    cargoPackageCount: cargoMetadata.packages.length,
  });
}

export function unifiedDiff(actual, expected, label = "THIRD_PARTY.md") {
  if (actual === expected) return "";
  const oldLines = splitLines(actual);
  const newLines = splitLines(expected);
  let prefix = 0;
  while (
    prefix < oldLines.length &&
    prefix < newLines.length &&
    oldLines[prefix] === newLines[prefix]
  ) {
    prefix += 1;
  }
  let suffix = 0;
  while (
    suffix < oldLines.length - prefix &&
    suffix < newLines.length - prefix &&
    oldLines[oldLines.length - 1 - suffix] === newLines[newLines.length - 1 - suffix]
  ) {
    suffix += 1;
  }
  const context = 3;
  const oldStart = Math.max(0, prefix - context);
  const newStart = Math.max(0, prefix - context);
  const oldChangedEnd = oldLines.length - suffix;
  const newChangedEnd = newLines.length - suffix;
  const oldEnd = Math.min(oldLines.length, oldChangedEnd + context);
  const newEnd = Math.min(newLines.length, newChangedEnd + context);
  const output = [`--- ${label}`, `+++ ${label} (generated)`];
  const oldCount = oldEnd - oldStart;
  const newCount = newEnd - newStart;
  output.push(`@@ -${oldStart + 1},${oldCount} +${newStart + 1},${newCount} @@`);
  for (const line of oldLines.slice(oldStart, prefix)) output.push(` ${line}`);
  for (const line of oldLines.slice(prefix, oldChangedEnd)) output.push(`-${line}`);
  for (const line of newLines.slice(prefix, newChangedEnd)) output.push(`+${line}`);
  for (const line of oldLines.slice(oldChangedEnd, oldEnd)) output.push(` ${line}`);
  return `${output.join("\n")}\n`;
}

function parseArgs(argv) {
  let check = false;
  let target = thirdPartyPath;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--check") {
      check = true;
    } else if (argument === "--output" || argument === "--file") {
      const value = argv[index + 1];
      if (!value) throw new Error(`${argument} requires a path.`);
      target = resolve(process.cwd(), value);
      index += 1;
    } else {
      throw new Error(`Unknown argument ${argument}.`);
    }
  }
  return { check, target };
}

export function writeGeneratedDocument(
  target,
  current,
  rendered,
  { write = writeFileSync, log = console.log } = {},
) {
  if (rendered.markdown === current) return false;
  for (const message of rendered.migrationMessages) log(message);
  write(target, rendered.markdown, "utf8");
  return true;
}

export async function generateFile({
  target = thirdPartyPath,
  check = false,
  readText = readFileSync,
  cargoMetadataReader = readCargoMetadata,
  npmRecordsCollector = collectNpmRecords,
} = {}) {
  const current = readText(target, "utf8");
  const lockfileText = readText(resolve(repoRoot, "pnpm-lock.yaml"), "utf8");
  const cargoMetadata = await cargoMetadataReader();
  const rustRecords = collectCargoRecords(cargoMetadata);
  const npmRecords = await npmRecordsCollector(lockfileText);
  const rendered = updateCountsAndRender(current, cargoMetadata, rustRecords, npmRecords);
  if (rendered.markdown === current) {
    if (check) console.log(`${target} is in sync.`);
    return { changed: false, rustRecords, npmRecords, ...rendered };
  }

  if (check) {
    process.stderr.write(unifiedDiff(current, rendered.markdown, target));
    return { changed: true, rustRecords, npmRecords, ...rendered };
  }

  writeGeneratedDocument(target, current, rendered);
  console.log(
    `Wrote ${target}: ${rustRecords.length} Rust records, ${npmRecords.length} npm records.`,
  );
  return { changed: true, rustRecords, npmRecords, ...rendered };
}

export async function main(argv = process.argv.slice(2)) {
  try {
    const options = parseArgs(argv);
    const result = await generateFile(options);
    if (options.check && result.changed) return 1;
    return 0;
  } catch (error) {
    console.error(`ERROR: ${errorMessage(error)}`);
    return 1;
  }
}

if (pathToFileURL(process.argv[1] ?? "").href === import.meta.url) {
  process.exitCode = await main();
}
