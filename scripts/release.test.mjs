import { describe, expect, it } from "vitest";
import {
  bumpVersion,
  extractChangelogSection,
  findVersionDrift,
  makeChangelogSection,
  parseCargoLockVersions,
  parseReleaseArgs,
  pickReleaseBaseTag,
} from "./release.mjs";

describe("bumpVersion", () => {
  it("increments the patch alone", () => {
    expect(bumpVersion("0.1.0", "patch")).toBe("0.1.1");
    expect(bumpVersion("0.1.9", "patch")).toBe("0.1.10");
  });

  it("increments the minor and zeroes the patch", () => {
    expect(bumpVersion("1.2.3", "minor")).toBe("1.3.0");
    expect(bumpVersion("0.1.9", "minor")).toBe("0.2.0");
  });

  it("increments the major and zeroes minor and patch", () => {
    expect(bumpVersion("1.2.3", "major")).toBe("2.0.0");
    expect(bumpVersion("0.1.0", "major")).toBe("1.0.0");
  });

  it("rejects a bump kind outside patch, minor, major", () => {
    expect(() => bumpVersion("0.1.0", "pre")).toThrow(/patch, minor or major/);
    expect(() => bumpVersion("0.1.0", "PATCH")).toThrow(/patch, minor or major/);
    expect(() => bumpVersion("0.1.0", "")).toThrow(/patch, minor or major/);
    expect(() => bumpVersion("0.1.0", undefined)).toThrow(/patch, minor or major/);
  });

  it("rejects a current version that is not X.Y.Z", () => {
    for (const current of ["1.2", "1.2.3-rc.1", "v1.2.3", "1.2.3.4", "0.1", "", "one.two.three"]) {
      expect(() => bumpVersion(current, "patch")).toThrow(/X\.Y\.Z/);
    }
  });
});

describe("makeChangelogSection", () => {
  const commits = [
    { hash: "0123456789abcdef", subject: "Newest change" },
    { hash: "abcdef0123456789", subject: "Oldest change" },
  ];

  it("renders heading, date, one Changes group and seven-character hashes", () => {
    expect(makeChangelogSection("0.2.0", "2026-10-07", commits)).toBe(
      [
        "## [0.2.0] - 2026-10-07",
        "",
        "### Changes",
        "- Newest change (0123456)",
        "- Oldest change (abcdef0)",
      ].join("\n"),
    );
  });

  it("keeps subjects verbatim, including parentheses and version-like text", () => {
    const section = makeChangelogSection("0.2.0", "2026-10-07", [
      { hash: "1234567", subject: "Fix the (double) counted v1.2.3 parser" },
    ]);
    expect(section).toContain("- Fix the (double) counted v1.2.3 parser (1234567)");
  });

  it("renders a first release's whole history when no tag set a range", () => {
    const history = [
      { hash: "1111111", subject: "Start the workspace" },
      { hash: "2222222", subject: "Add the daemon" },
      { hash: "3333333", subject: "Ship the installer" },
    ];
    const section = makeChangelogSection("0.1.0", "2026-01-01", history);
    const bullets = section.split("\n").filter((line) => line.startsWith("- "));
    expect(bullets).toHaveLength(3);
    expect(section.indexOf("Start the workspace")).toBeLessThan(
      section.indexOf("Ship the installer"),
    );
  });
});

describe("pickReleaseBaseTag", () => {
  it("returns null when no reachable tag is a release version", () => {
    expect(pickReleaseBaseTag([])).toBeNull();
    expect(pickReleaseBaseTag(["bulk-close-pre-squash", "u4-pre-squash"])).toBeNull();
  });

  it("picks the highest release version, numerically not lexically", () => {
    expect(pickReleaseBaseTag(["v0.9.0", "u4-pre-squash", "v0.10.0", "v0.2.0"])).toBe("v0.10.0");
  });

  it("ignores malformed tags instead of ranking them", () => {
    expect(pickReleaseBaseTag(["v1.2", "1.2.3", "v1.2.3-rc.1", "release-v1.2.3"])).toBeNull();
    expect(pickReleaseBaseTag(["v1.2.3-rc.1", "v1.2.2"])).toBe("v1.2.2");
  });
});

describe("extractChangelogSection", () => {
  const changelog = [
    "# Changelog",
    "",
    "## [Unreleased]",
    "",
    "## [0.2.0] - 2026-10-07",
    "",
    "### Changes",
    "- Second release (aaaaaaa)",
    "",
    "## [0.1.0] - 2026-09-01",
    "",
    "### Changes",
    "- First release (bbbbbbb)",
    "",
  ].join("\n");

  it("returns one version's section without its neighbours", () => {
    const section = extractChangelogSection(changelog, "0.2.0");
    expect(section).toBe(
      ["## [0.2.0] - 2026-10-07", "", "### Changes", "- Second release (aaaaaaa)"].join("\n"),
    );
    expect(section).not.toContain("[Unreleased]");
    expect(section).not.toContain("0.1.0");
    expect(section).not.toContain("bbbbbbb");
  });

  it("returns the trailing section down to the end of the file", () => {
    expect(extractChangelogSection(changelog, "0.1.0")).toContain("- First release (bbbbbbb)");
    expect(extractChangelogSection(changelog, "0.1.0")).not.toContain("aaaaaaa");
  });

  it("does not mistake a longer version for the one asked for", () => {
    expect(extractChangelogSection("## [0.1.10] - 2026-01-01\n", "0.1.1")).toBeNull();
  });

  it("returns null when the version is absent", () => {
    expect(extractChangelogSection(changelog, "9.9.9")).toBeNull();
  });
});

describe("parseReleaseArgs", () => {
  it("accepts exactly one bump kind as release mode", () => {
    expect(parseReleaseArgs(["patch"])).toEqual({ mode: "release", kind: "patch" });
    expect(parseReleaseArgs(["minor"])).toEqual({ mode: "release", kind: "minor" });
    expect(parseReleaseArgs(["major"])).toEqual({ mode: "release", kind: "major" });
  });

  it("accepts the notes form with a vX.Y.Z tag and an output path", () => {
    expect(parseReleaseArgs(["notes", "v0.1.1", "release-notes.md"])).toEqual({
      mode: "notes",
      version: "v0.1.1",
      output: "release-notes.md",
    });
  });

  it("rejects anything that is not one of the two CLI forms", () => {
    const invalid = [
      [],
      ["pre"],
      ["PATCH"],
      ["patch", "minor"],
      ["v0.1.1"],
      ["notes", "v0.1.1"],
      ["notes", "0.1.1", "release-notes.md"],
      ["notes", "v0.1", "release-notes.md"],
      ["notes", "v0.1.1", ""],
      ["notes", "v0.1.1", "release-notes.md", "extra"],
    ];
    for (const args of invalid) {
      expect(() => parseReleaseArgs(args), JSON.stringify(args)).toThrow(/usage/);
    }
  });
});

describe("findVersionDrift", () => {
  it("refuses nothing when every source matches the canonical version", () => {
    const sources = {
      "src-tauri/tauri.conf.json": "0.1.0",
      "package.json": "0.1.0",
      "src-tauri/Cargo.toml": "0.1.0",
      "crates/oracle-core/Cargo.toml": "0.1.0",
    };
    expect(findVersionDrift(sources, "0.1.0")).toEqual([]);
  });

  it("names every source that disagrees with the canonical version", () => {
    const sources = {
      "src-tauri/tauri.conf.json": "0.1.0",
      "package.json": "0.2.0",
      "src-tauri/Cargo.toml": "0.1.0",
      "crates/oracle-core/Cargo.toml": "0.1.1",
    };
    expect(findVersionDrift(sources, "0.1.0")).toEqual([
      "package.json",
      "crates/oracle-core/Cargo.toml",
    ]);
  });

  it("refuses a Cargo.lock entry that is behind or missing", () => {
    expect(findVersionDrift({ devboule: "0.1.0", "polis-backend": undefined }, "0.1.1")).toEqual([
      "devboule",
      "polis-backend",
    ]);
  });
});

describe("parseCargoLockVersions", () => {
  it("reads each package's version out of lock blocks", () => {
    const lock = [
      "version = 4",
      "",
      "[[package]]",
      'name = "devboule"',
      'version = "0.1.1"',
      "",
      "[[package]]",
      'name = "oracle-core"',
      'version = "0.1.1"',
      "",
    ].join("\n");
    expect(parseCargoLockVersions(lock)).toEqual({ devboule: "0.1.1", "oracle-core": "0.1.1" });
  });
});
