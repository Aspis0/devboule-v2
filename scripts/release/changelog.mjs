// The changelog's release sections: build one, find one, validate one,
// insert one under [Unreleased]. Pure text in, text out.

export function makeChangelogSection(version, date, commits) {
  const bullets = commits.map(({ hash, subject }) => `- ${subject} (${hash.slice(0, 7)})`);
  return [`## [${version}] - ${date}`, "", "### Changes", ...bullets].join("\n");
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
  const date = /\d{4}-\d{2}-\d{2}$/.exec(lines[0])[0];
  const [year, month, day] = date.split("-").map(Number);
  const probe = new Date(Date.UTC(year, month - 1, day));
  // Date.UTC maps years 0-99 into 1900-1999, so the round-trip must compare
  // against the year it actually built, not the year written on the heading.
  const builtYear = year < 100 ? 1900 + year : year;
  if (
    probe.getUTCFullYear() !== builtYear ||
    probe.getUTCMonth() !== month - 1 ||
    probe.getUTCDate() !== day
  ) {
    throw new Error(`section date is not a real calendar date: ${date}`);
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

export function insertChangelogSection(changelog, section) {
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
