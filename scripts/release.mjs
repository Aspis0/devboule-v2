import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Version arithmetic, changelog sections and the release-base tag choice —
// pure so scripts/release.test.mjs can exercise them without a repository.

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
function parseReleaseTag(tag) {
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

export function makeChangelogSection(version, date, commits) {
  const bullets = commits.map(({ hash, subject }) => `- ${subject} (${hash.slice(0, 7)})`);
  return [`## [${version}] - ${date}`, "", "### Changes", ...bullets].join("\n");
}

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

export function extractChangelogSection(changelog, version) {
  const lines = changelog.split(/\r?\n/);
  const pattern = sectionHeadingPattern(version);
  const starts = [];
  lines.forEach((line, index) => {
    if (pattern.test(line)) {
      starts.push(index);
    }
  });
  if (starts.length > 1) {
    throw new Error(
      `CHANGELOG carries ${starts.length} sections for ${version}, expected exactly one`,
    );
  }
  if (starts.length === 0) {
    return null;
  }
  const start = starts[0];
  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    if (lines[index].startsWith("## ")) {
      end = index;
      break;
    }
  }
  return lines.slice(start, end).join("\n").replace(/\s+$/, "");
}

// One release section, exactly as the brief defines it: the dated heading,
// a blank line, `### Changes`, and nothing but `- ` bullets below.
export function assertValidSection(section, version) {
  const lines = section.split(/\r?\n/);
  if (lines[0] === undefined || !sectionHeadingPattern(version).test(lines[0])) {
    throw new Error(`section heading must be exactly "## [${version}] - YYYY-MM-DD"`);
  }
  if (lines[1] !== "" || lines[2] !== "### Changes") {
    throw new Error(`section must put a blank line and "### Changes" under its heading`);
  }
  const body = lines.slice(3).filter((line) => line !== "");
  if (body.length === 0) {
    throw new Error(`section for ${version} has no changes to release`);
  }
  const notBullet = body.find((line) => !line.startsWith("- "));
  if (notBullet !== undefined) {
    throw new Error(`section body may only hold "- " bullets, found: ${JSON.stringify(notBullet)}`);
  }
}

function sectionHeadingPattern(version) {
  const escaped = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`^## \\[${escaped}\\] - \\d{4}-\\d{2}-\\d{2}$`);
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

export function parseReleaseArgs(args) {
  if (args.length === 1 && (args[0] === "patch" || args[0] === "minor" || args[0] === "major")) {
    return { mode: "release", kind: args[0] };
  }
  if (
    args.length === 3 &&
    args[0] === "notes" &&
    parseReleaseTag(args[1]) !== null &&
    args[2] !== ""
  ) {
    return { mode: "notes", version: args[1], output: args[2] };
  }
  if (args.length === 2 && args[0] === "check-tag" && parseReleaseTag(args[1]) !== null) {
    return { mode: "check-tag", version: args[1] };
  }
  throw new Error(
    "usage: release.mjs <patch|minor|major> | release.mjs notes <vX.Y.Z> <file> | release.mjs check-tag <vX.Y.Z>",
  );
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

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CHANGELOG_PATH = join(repoRoot, "CHANGELOG.md");
const CARGO_LOCK_PATH = join(repoRoot, "Cargo.lock");

// tauri.conf.json is canonical because Tauri takes the shipped app version
// from it; the other eight files must equal it or one release would carry
// several versions at once.
const VERSION_SOURCES = [
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
const LOCK_PACKAGES = [
  "devboule",
  "devboule-augur",
  "devboule-daemon",
  "devboule-plugin-rpc",
  "devboule-protocol",
  "oracle-core",
  "polis-backend",
];
const BOT_IDENTITY = [
  "-c",
  "user.name=devboule-bot",
  "-c",
  "user.email=devboule-bot@devboule.invalid",
];
// Exactly what a release commit may change: staged, checked and restored as
// one set.
const RELEASE_PATHS = [...VERSION_SOURCES, "Cargo.lock", "CHANGELOG.md"];

function git(args, options = {}) {
  return execFileSync("git", args, { cwd: repoRoot, encoding: "utf8", ...options });
}

function readVersionOf(fullPath) {
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
function writeVersionTo(fullPath, version) {
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

function readSourceVersions() {
  const versions = {};
  for (const source of VERSION_SOURCES) {
    versions[source] = readVersionOf(join(repoRoot, source));
  }
  return versions;
}

function utcDate() {
  return new Date().toISOString().slice(0, 10);
}

function insertChangelogSection(changelog, section) {
  // .gitattributes pins the working tree to LF, so the marker is exact.
  const marker = "\n## [Unreleased]\n";
  const at = changelog.indexOf(marker);
  if (at === -1) {
    throw new Error("CHANGELOG.md has no ## [Unreleased] heading");
  }
  const insertAt = at + marker.length;
  const body = changelog.slice(insertAt).replace(/^\n+/, "");
  const reviewed = section.replace(/\s+$/, "");
  return `${changelog.slice(0, insertAt)}\n${reviewed}\n\n${body}`.replace(/\n+$/, "\n");
}

function assertReviewed(reviewed, version) {
  const section = extractChangelogSection(reviewed, version);
  if (section === null) {
    throw new Error(`the reviewed notes carry no "## [${version}] - YYYY-MM-DD" heading`);
  }
  assertValidSection(section, version);
}

function reviewEditor() {
  let editor;
  try {
    editor = git(["var", "GIT_EDITOR"]).trim();
  } catch {
    editor = "";
  }
  if (editor === "") {
    return process.platform === "win32" ? "notepad.exe" : "vi";
  }
  if (process.platform === "win32" && (editor === "vi" || editor === "vim")) {
    // git var reports the POSIX default when nothing is configured, and that
    // default is not on PATH in a Windows shell — notepad.exe is the fallback.
    return "notepad.exe";
  }
  return editor;
}

function reviewDraft(draft, version) {
  const fromFile = process.env.RELEASE_NOTES_FILE;
  let reviewed;
  if (fromFile !== undefined && fromFile !== "") {
    // Scripted runs and tests hand the reviewed block over in a file instead
    // of an interactive editor; the validation below still applies, and the
    // skip is announced loudly so it can never pass as a human review.
    console.log(`editor review skipped: using RELEASE_NOTES_FILE=${fromFile}`);
    reviewed = readFileSync(fromFile, "utf8");
  } else {
    const draftPath = join(mkdtempSync(join(tmpdir(), "devboule-release-")), "release-notes.md");
    writeFileSync(draftPath, draft);
    const result = spawnSync(`${reviewEditor()} "${draftPath}"`, {
      shell: true,
      stdio: "inherit",
      cwd: repoRoot,
    });
    if (result.status !== 0) {
      throw new Error(
        `the editor ended without exit code 0 (status ${result.status}); nothing was changed`,
      );
    }
    reviewed = readFileSync(draftPath, "utf8");
  }
  assertReviewed(reviewed, version);
  return reviewed;
}

// Once the source writes have begun, a failure must not leave a half-written
// release behind — every gate expects a clean tree — so the release paths go
// back to HEAD (index included, since the failure can come after `git add`).
function restoreReleasePaths(git) {
  const dirty = git(["diff", "--name-only", "HEAD", "--", ...RELEASE_PATHS]).trim();
  git(["checkout", "HEAD", "--", ...RELEASE_PATHS]);
  console.error(
    dirty === "" ? "the release paths were already unchanged" : `restored release paths:\n${dirty}`,
  );
}

function runRelease(kind) {
  const canonical = readVersionOf(join(repoRoot, "src-tauri/tauri.conf.json"));
  const next = bumpVersion(canonical, kind);

  const branch = git(["rev-parse", "--abbrev-ref", "HEAD"]).trim();
  if (branch !== "main") {
    throw new Error(`release only runs on main, currently on ${branch}`);
  }
  if (git(["status", "--porcelain=v1"]).trim() !== "") {
    throw new Error("the working tree is not clean");
  }
  // Fetch first so the origin/main comparison and the tag check below both
  // answer about the same freshly fetched history.
  git(["fetch", "origin", "--tags"]);
  const head = git(["rev-parse", "HEAD"]).trim();
  const originMain = git(["rev-parse", "origin/main"]).trim();
  if (head !== originMain) {
    throw new Error(`HEAD ${head} is not origin/main ${originMain}`);
  }

  const drift = findVersionDrift(readSourceVersions(), canonical);
  if (drift.length > 0) {
    throw new Error(`version drift from the canonical ${canonical}: ${drift.join(", ")}`);
  }

  const tag = `v${next}`;
  if (git(["tag", "-l", tag]).trim() !== "") {
    throw new Error(`${tag} already exists locally or on origin (tags were fetched)`);
  }

  const baseTag = pickReleaseBaseTag(
    git(["tag", "--merged", "HEAD"])
      .split(/\r?\n/)
      .filter((name) => name !== ""),
  );
  const range = baseTag === null ? [] : [`${baseTag}..HEAD`];
  const log = git(["log", "--no-merges", "--format=%h%x09%s", ...range]).trim();
  if (log === "") {
    throw new Error("no non-merge commits to report");
  }
  const commits = log.split(/\r?\n/).map((line) => {
    const tab = line.indexOf("\t");
    return { hash: line.slice(0, tab), subject: line.slice(tab + 1) };
  });

  // The reviewed block is settled before any tracked file is touched.
  const reviewed = reviewDraft(makeChangelogSection(next, utcDate(), commits), next);

  const subject = `Release ${tag}`;
  try {
    for (const source of VERSION_SOURCES) {
      writeVersionTo(join(repoRoot, source), next);
    }
    writeFileSync(
      CHANGELOG_PATH,
      insertChangelogSection(readFileSync(CHANGELOG_PATH, "utf8"), reviewed),
    );

    execFileSync("cargo", ["update", "--workspace", "--offline"], {
      cwd: repoRoot,
      stdio: "inherit",
    });

    const written = findVersionDrift(readSourceVersions(), next);
    if (written.length > 0) {
      throw new Error(`sources did not all land on ${next}: ${written.join(", ")}`);
    }
    const lock = parseCargoLockVersions(readFileSync(CARGO_LOCK_PATH, "utf8"));
    const lockVersions = {};
    for (const name of LOCK_PACKAGES) {
      lockVersions[name] = lock[name];
    }
    const lockDrift = findVersionDrift(lockVersions, next);
    if (lockDrift.length > 0) {
      throw new Error(`Cargo.lock did not all land on ${next}: ${lockDrift.join(", ")}`);
    }
    git(["diff", "--check"]);

    git(["add", "--", ...RELEASE_PATHS]);
    git([...BOT_IDENTITY, "commit", "-m", subject]);
  } catch (error) {
    try {
      restoreReleasePaths(git);
    } catch (restoreError) {
      console.error(`restoring the release paths failed: ${restoreError.message}`);
    }
    throw error;
  }

  const sha = git(["rev-parse", "HEAD"]).trim();
  try {
    git([...BOT_IDENTITY, "tag", "-a", tag, "-m", subject]);
  } catch (error) {
    console.error(`the release commit ${sha} exists, but the ${tag} tag was not created.`);
    console.error(`retry the tag:  git tag -a ${tag} -m "${subject}" ${sha}`);
    console.error(`or drop it:     git reset --hard origin/main`);
    throw error;
  }

  // One atomic push: if main moved since the preflight, the tag must not
  // reach the remote on its own without the commit that labels.
  console.log(`git push --atomic origin main ${tag}`);
}

function runNotes(tag, outputPath) {
  const version = tag.slice(1);
  const section = extractChangelogSection(readFileSync(CHANGELOG_PATH, "utf8"), version);
  if (section === null) {
    throw new Error(`CHANGELOG.md has no section for ${tag}`);
  }
  assertValidSection(section, version);
  writeFileSync(outputPath, `${section}\n`);
}

// CI's admission check: the pushed tag must be the app version and must have
// a changelog section to publish, so a tag cannot label another build.
function runCheckTag(tag) {
  const canonical = readVersionOf(join(repoRoot, "src-tauri/tauri.conf.json"));
  if (tag !== `v${canonical}`) {
    throw new Error(
      `${tag} does not match the app version v${canonical} in src-tauri/tauri.conf.json`,
    );
  }
  const section = extractChangelogSection(readFileSync(CHANGELOG_PATH, "utf8"), canonical);
  if (section === null) {
    throw new Error(`CHANGELOG.md has no section for ${tag}`);
  }
  assertValidSection(section, canonical);
}

if (import.meta.main) {
  try {
    const request = parseReleaseArgs(process.argv.slice(2));
    if (request.mode === "release") {
      runRelease(request.kind);
    } else if (request.mode === "check-tag") {
      runCheckTag(request.version);
    } else {
      runNotes(request.version, request.output);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
