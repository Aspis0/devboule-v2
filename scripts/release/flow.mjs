// The release commands themselves: the gated release flow with its rollback
// and recovery, the CI tag admission check, and the notes extractor. Every
// external command goes through the injected runner so the committed test
// fixture can stand in for cargo and a temp repository for the real repo.

import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  LOCK_PACKAGES,
  VERSION_SOURCES,
  bumpVersion,
  findVersionDrift,
  parseCargoLockVersions,
  pickReleaseBaseTag,
  readSourceVersions,
  readVersionOf,
  writeVersionTo,
} from "./versions.mjs";
import {
  assertValidSection,
  extractChangelogSection,
  insertChangelogSection,
  makeChangelogSection,
} from "./changelog.mjs";

// scripts/release/flow.mjs sits two levels below the repository root.
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const CHANGELOG_PATH = join(repoRoot, "CHANGELOG.md");
const BOT_IDENTITY = [
  "-c",
  "user.name=devboule-bot",
  "-c",
  "user.email=devboule-bot@devboule.invalid",
];
// Exactly what a release commit may change: staged, checked and restored as
// one set.
const RELEASE_PATHS = [...VERSION_SOURCES, "Cargo.lock", "CHANGELOG.md"];

function utcDate() {
  return new Date().toISOString().slice(0, 10);
}

function assertReviewed(reviewed, version) {
  const section = extractChangelogSection(reviewed, version);
  if (section === null) {
    throw new Error(`the reviewed notes carry no "## [${version}] - YYYY-MM-DD" heading`);
  }
  assertValidSection(section, version);
}

function reviewEditor(root) {
  let editor;
  try {
    editor = execFileSync("git", ["var", "GIT_EDITOR"], { cwd: root, encoding: "utf8" }).trim();
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

function reviewDraft(draft, version, root) {
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
    const result = spawnSync(`${reviewEditor(root)} "${draftPath}"`, {
      shell: true,
      stdio: "inherit",
      cwd: root,
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

// options.root and options.runner are the seam the committed test fixture
// uses: the flow runs against a temp repository with cargo stubbed, so no
// test can ever reach the real repo or the real lockfile.
export function runRelease(kind, options = {}) {
  const root = options.root ?? repoRoot;
  const runner = options.runner ?? execFileSync;
  const git = (args, gitOptions = {}) =>
    runner("git", args, { cwd: root, encoding: "utf8", ...gitOptions });
  const changelogPath = join(root, "CHANGELOG.md");
  const cargoLockPath = join(root, "Cargo.lock");

  const canonical = readVersionOf(join(root, "src-tauri/tauri.conf.json"));
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

  const drift = findVersionDrift(readSourceVersions(root), canonical);
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
  const reviewed = reviewDraft(makeChangelogSection(next, utcDate(), commits), next, root);

  const subject = `Release ${tag}`;
  try {
    for (const source of VERSION_SOURCES) {
      writeVersionTo(join(root, source), next);
    }
    writeFileSync(
      changelogPath,
      insertChangelogSection(readFileSync(changelogPath, "utf8"), reviewed),
    );

    runner("cargo", ["update", "--workspace", "--offline"], {
      cwd: root,
      stdio: "inherit",
    });

    const written = findVersionDrift(readSourceVersions(root), next);
    if (written.length > 0) {
      throw new Error(`sources did not all land on ${next}: ${written.join(", ")}`);
    }
    const lock = parseCargoLockVersions(readFileSync(cargoLockPath, "utf8"));
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
    console.error(`first check that git rev-parse HEAD still equals ${sha}`);
    console.error(`retry the tag:  git tag -a ${tag} -m "${subject}" ${sha}`);
    console.error(`or drop the release commit: git reset --keep ${sha}^`);
    throw error;
  }

  // One atomic push: if main moved since the preflight, the tag must not
  // reach the remote on its own without the commit that labels.
  console.log(`git push --atomic origin main ${tag}`);
}

export function runNotes(tag, outputPath) {
  const version = tag.slice(1);
  const section = extractChangelogSection(readFileSync(CHANGELOG_PATH, "utf8"), version);
  if (section === null) {
    throw new Error(`CHANGELOG.md has no section for ${tag}`);
  }
  assertValidSection(section, version);
  writeFileSync(outputPath, `${section}\n`);
}

// CI's admission check: the tag must name the app version, every version
// source and workspace lock entry must equal it, and a changelog section must
// exist to publish — a tag cannot label another build.
export function runCheckTag(tag, options = {}) {
  const root = options.root ?? repoRoot;
  const canonical = readVersionOf(join(root, "src-tauri/tauri.conf.json"));
  if (tag !== `v${canonical}`) {
    throw new Error(
      `${tag} does not match the app version v${canonical} in src-tauri/tauri.conf.json`,
    );
  }
  const inventory = readSourceVersions(root);
  const lock = parseCargoLockVersions(readFileSync(join(root, "Cargo.lock"), "utf8"));
  for (const name of LOCK_PACKAGES) {
    inventory[`Cargo.lock (${name})`] = lock[name];
  }
  const drift = findVersionDrift(inventory, canonical);
  if (drift.length > 0) {
    throw new Error(`version drift from ${tag}: ${drift.join(", ")}`);
  }
  const section = extractChangelogSection(
    readFileSync(join(root, "CHANGELOG.md"), "utf8"),
    canonical,
  );
  if (section === null) {
    throw new Error(`CHANGELOG.md has no section for ${tag}`);
  }
  assertValidSection(section, canonical);
}
