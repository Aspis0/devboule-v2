import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Version arithmetic, changelog sections and the release-base tag choice —
// pure so scripts/release.test.mjs can exercise them without a repository.

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;
const RELEASE_TAG = /^v(\d+)\.(\d+)\.(\d+)$/;
const JSON_VERSION_LINE = /^( {2}"version": ")([^"]*)(")/m;
const TOML_VERSION_LINE = /^(version = ")([^"]*)(")/m;

export function bumpVersion(current, kind) {
  if (kind !== "patch" && kind !== "minor" && kind !== "major") {
    throw new Error(`bump kind must be patch, minor or major, got ${JSON.stringify(kind)}`);
  }
  const match = SEMVER.exec(current);
  if (match === null) {
    throw new Error(`current version is not X.Y.Z: ${JSON.stringify(current)}`);
  }
  let major = Number(match[1]);
  let minor = Number(match[2]);
  let patch = Number(match[3]);
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
  return `${major}.${minor}.${patch}`;
}

export function makeChangelogSection(version, date, commits) {
  const bullets = commits.map(({ hash, subject }) => `- ${subject} (${hash.slice(0, 7)})`);
  return [`## [${version}] - ${date}`, "", "### Changes", ...bullets].join("\n");
}

export function extractChangelogSection(changelog, version) {
  const heading = `## [${version}]`;
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((line) => line === heading || line.startsWith(`${heading} `));
  if (start === -1) {
    return null;
  }
  let end = lines.length;
  for (let index = start + 1; index < lines.length; index += 1) {
    if (lines[index].startsWith("## ")) {
      end = index;
      break;
    }
  }
  return lines.slice(start, end).join("\n").replace(/\s+$/, "");
}

// The tag the changelog range stops at: the highest vX.Y.Z among the tags
// merged into HEAD. Label tags never qualify, so a repository whose only tags
// are labels has no base and the first release reports the whole history.
export function pickReleaseBaseTag(tagNames) {
  let best = null;
  let bestKey = null;
  for (const name of tagNames) {
    const match = RELEASE_TAG.exec(name);
    if (match === null) {
      continue;
    }
    const key = [Number(match[1]), Number(match[2]), Number(match[3])];
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
    /^v\d+\.\d+\.\d+$/.test(args[1]) &&
    args[2] !== ""
  ) {
    return { mode: "notes", version: args[1], output: args[2] };
  }
  throw new Error("usage: release.mjs <patch|minor|major> | release.mjs notes <vX.Y.Z> <file>");
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

function git(args, options = {}) {
  return execFileSync("git", args, { cwd: repoRoot, encoding: "utf8", ...options });
}

function readVersionOf(fullPath) {
  const text = readFileSync(fullPath, "utf8");
  const match = (fullPath.endsWith(".json") ? JSON_VERSION_LINE : TOML_VERSION_LINE).exec(text);
  if (match === null) {
    throw new Error(`${fullPath}: no version line`);
  }
  return match[2];
}

// Narrow line replacement: tauri.conf.json is JSON-with-comments and the
// Cargo manifests carry dependency lines, so reserializing either would churn
// the whole file.
function writeVersionTo(fullPath, version) {
  const text = readFileSync(fullPath, "utf8");
  const pattern = fullPath.endsWith(".json") ? JSON_VERSION_LINE : TOML_VERSION_LINE;
  if (pattern.exec(text) === null) {
    throw new Error(`${fullPath}: no version line`);
  }
  writeFileSync(fullPath, text.replace(pattern, `$1${version}$3`));
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
    throw new Error(`the reviewed notes carry no ## [${version}] heading`);
  }
  const bodyStart = section.indexOf("\n");
  if (bodyStart === -1 || section.slice(bodyStart + 1).trim() === "") {
    throw new Error(`the reviewed notes for v${version} are empty under the heading`);
  }
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
    // of an interactive editor; the validation below still applies.
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

  const subject = `Release ${tag}`;
  git(["add", "--", ...VERSION_SOURCES, "Cargo.lock", "CHANGELOG.md"]);
  git([...BOT_IDENTITY, "commit", "-m", subject]);
  git([...BOT_IDENTITY, "tag", "-a", tag, "-m", subject]);

  console.log(`git push origin main ${tag}`);
}

function runNotes(tag, outputPath) {
  const section = extractChangelogSection(readFileSync(CHANGELOG_PATH, "utf8"), tag.slice(1));
  if (section === null) {
    throw new Error(`CHANGELOG.md has no section for ${tag}`);
  }
  writeFileSync(outputPath, `${section}\n`);
}

if (import.meta.main) {
  try {
    const request = parseReleaseArgs(process.argv.slice(2));
    if (request.mode === "release") {
      runRelease(request.kind);
    } else {
      runNotes(request.version, request.output);
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
