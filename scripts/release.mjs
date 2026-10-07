// Version arithmetic, changelog sections and the release-base tag choice —
// pure so scripts/release.test.mjs can exercise them without a repository.

const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/;
const RELEASE_TAG = /^v(\d+)\.(\d+)\.(\d+)$/;

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

// The tag the changelog range starts after: the highest vX.Y.Z among the tags
// merged into HEAD. Label tags that are not release versions never qualify, so
// a repository whose only tags are labels has no base and releases everything
// reachable since the beginning.
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
