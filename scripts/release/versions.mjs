// SemVer parsing and bumping, the nine version sources, and the Cargo.lock
// workspace inventory — everything that answers "what version is where".

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// Strict SemVer: no leading zeroes, no component above Number.MAX_SAFE_INTEGER
// (a larger component would silently lose precision in any arithmetic).
const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const RELEASE_TAG = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const JSON_VERSION_LINE = /^( {2}"version": ")([^"]*)(")/m;

function versionComponents(text, pattern, label) {
  const match = pattern.exec(text);
  if (match === null) {
    throw new Error(`${label} is not X.Y.Z: ${JSON.stringify(text)}`);
  }
  const parts = match.slice(1).map(Number);
  if (!parts.every(Number.isSafeInteger)) {
    throw new Error(`${label} exceeds Number.MAX_SAFE_INTEGER: ${JSON.stringify(text)}`);
  }
  return parts;
}

// Null for anything that is not a strict, precisely representable release
// tag: malformed tags are not release bases and not valid CLI arguments.
export function parseReleaseTag(tag) {
  const match = RELEASE_TAG.exec(tag);
  if (match === null) {
    return null;
  }
  const parts = match.slice(1).map(Number);
  return parts.every(Number.isSafeInteger) ? parts : null;
}

export function bumpVersion(current, kind) {
  if (kind !== "patch" && kind !== "minor" && kind !== "major") {
    throw new Error(`bump kind must be patch, minor or major, got ${JSON.stringify(kind)}`);
  }
  let [major, minor, patch] = versionComponents(current, SEMVER, "current version");
  if (kind === "patch") {
    patch += 1;
  } else if (kind === "minor") {
    minor += 1;
    patch = 0;
  } else {
    major += 1;
    minor = 0;
    patch = 0;
  }
  if (![major, minor, patch].every(Number.isSafeInteger)) {
    throw new Error(`bumping ${JSON.stringify(current)} exceeds Number.MAX_SAFE_INTEGER`);
  }
  return `${major}.${minor}.${patch}`;
}

// The tag the changelog range stops at: the highest vX.Y.Z among the tags
// merged into HEAD. Label tags never qualify, so a repository whose only tags
// are labels has no base and the first release reports the whole history.
export function pickReleaseBaseTag(tagNames) {
  let best = null;
  let bestKey = null;
  for (const name of tagNames) {
    const key = parseReleaseTag(name);
    if (key === null) {
      continue;
    }
    if (bestKey === null || compare(key, bestKey) > 0) {
      best = name;
      bestKey = key;
    }
  }
  return best;
}

function compare(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) {
      return left[index] - right[index];
    }
  }
  return 0;
}

export function findVersionDrift(versions, canonical) {
  return Object.keys(versions).filter((label) => versions[label] !== canonical);
}

export function parseCargoLockVersions(text) {
  const versions = {};
  for (const block of text.split(/\[\[package\]\]/).slice(1)) {
    const name = /^name = "([^"]+)"/m.exec(block);
    const version = /^version = "([^"]+)"/m.exec(block);
    if (name !== null && version !== null) {
      versions[name[1]] = version[1];
    }
  }
  return versions;
}

// tauri.conf.json is canonical because Tauri takes the shipped app version
// from it; the other eight files must equal it or one release would carry
// several versions at once.
export const VERSION_SOURCES = [
  "src-tauri/tauri.conf.json",
  "package.json",
  "src-tauri/Cargo.toml",
  "crates/devboule-augur/Cargo.toml",
  "crates/devboule-daemon/Cargo.toml",
  "crates/devboule-plugin-rpc/Cargo.toml",
  "crates/devboule-protocol/Cargo.toml",
  "crates/oracle-core/Cargo.toml",
  "crates/polis-backend/Cargo.toml",
];
// The workspace packages `cargo update --workspace` rewrites inside Cargo.lock.
export const LOCK_PACKAGES = [
  "devboule",
  "devboule-augur",
  "devboule-daemon",
  "devboule-plugin-rpc",
  "devboule-protocol",
  "oracle-core",
  "polis-backend",
];

// Cargo manifests are TOML: the package version lives inside the [package]
// table, which is not necessarily the first table — a dependency table with
// its own `version =` key must never be read or rewritten as the package
// version. The working tree is LF (`.gitattributes`), as everywhere here.
function packageVersionMatch(text, fullPath) {
  const headers = [...text.matchAll(/^\[package\][ \t]*$/gm)];
  if (headers.length !== 1) {
    throw new Error(`${fullPath}: expected exactly one [package] table, found ${headers.length}`);
  }
  const bodyStart = headers[0].index + headers[0][0].length;
  const body = text.slice(bodyStart);
  const nextTable = /^\[[^\]\r\n]+\]/m.exec(body);
  const span = nextTable === null ? body : body.slice(0, nextTable.index);
  const versions = [...span.matchAll(/^version = "([^"]*)"/gm)];
  if (versions.length !== 1) {
    throw new Error(
      `${fullPath}: expected exactly one version in [package], found ${versions.length}`,
    );
  }
  return {
    index: bodyStart + versions[0].index,
    length: versions[0][0].length,
    value: versions[0][1],
  };
}

export function readVersionOf(fullPath) {
  const text = readFileSync(fullPath, "utf8");
  if (fullPath.endsWith(".json")) {
    const match = JSON_VERSION_LINE.exec(text);
    if (match === null) {
      throw new Error(`${fullPath}: no version line`);
    }
    return match[2];
  }
  return packageVersionMatch(text, fullPath).value;
}

// Narrow line replacement: tauri.conf.json is JSON-with-comments and the
// Cargo manifests carry dependency lines, so reserializing either would churn
// the whole file.
export function writeVersionTo(fullPath, version) {
  const text = readFileSync(fullPath, "utf8");
  if (fullPath.endsWith(".json")) {
    if (JSON_VERSION_LINE.exec(text) === null) {
      throw new Error(`${fullPath}: no version line`);
    }
    writeFileSync(fullPath, text.replace(JSON_VERSION_LINE, `$1${version}$3`));
    return;
  }
  const found = packageVersionMatch(text, fullPath);
  writeFileSync(
    fullPath,
    `${text.slice(0, found.index)}version = "${version}"${text.slice(found.index + found.length)}`,
  );
}

export function readSourceVersions(root) {
  const versions = {};
  for (const source of VERSION_SOURCES) {
    versions[source] = readVersionOf(join(root, source));
  }
  return versions;
}
