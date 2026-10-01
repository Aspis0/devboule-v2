import { describe, expect, it } from "vitest";
import { parseChatFilePath, scanChatFilePaths } from "./chatFilePaths";

const POSIX_ROOT = "/home/u/repo";
const WINDOWS_ROOT = "C:/repo";

describe("accepted paths", () => {
  it.each([
    { candidate: "src/app/App.tsx", root: POSIX_ROOT, relativePath: "src/app/App.tsx" },
    { candidate: "./x/y.ts", root: POSIX_ROOT, relativePath: "x/y.ts" },
    { candidate: "/home/u/repo/src/a.ts", root: POSIX_ROOT, relativePath: "src/a.ts" },
    { candidate: "C:/repo/src/a.ts", root: WINDOWS_ROOT, relativePath: "src/a.ts" },
    { candidate: "C:\\repo\\src\\a.ts", root: "C:\\repo", relativePath: "src/a.ts" },
    { candidate: "C:/repo/src/a.ts", root: "c:\\repo", relativePath: "src/a.ts" },
    { candidate: "c:/repo/src/a.ts", root: "C:/repo", relativePath: "src/a.ts" },
    { candidate: "C:/REPO/src/a.ts", root: WINDOWS_ROOT, relativePath: "src/a.ts" },
    { candidate: "c:/repo/Src/A.ts", root: "C:/REPO", relativePath: "Src/A.ts" },
    {
      candidate: String.raw`c:\users\u\Repo\src\a.ts`,
      root: String.raw`C:\Users\u\repo`,
      relativePath: "src/a.ts",
    },
    {
      candidate: String.raw`\\?\c:\users\u\Repo\Src\A.ts`,
      root: String.raw`C:\Users\u\repo`,
      relativePath: "Src/A.ts",
    },
    // A `..` that stays under the root is a plain relative hop.
    { candidate: "src/../app/x.ts", root: POSIX_ROOT, relativePath: "app/x.ts" },
  ])("resolves $candidate against $root", ({ candidate, root, relativePath }) => {
    expect(parseChatFilePath(candidate, root)).toEqual({ relativePath });
  });

  it.each([
    { candidate: "src/a.ts:12", root: POSIX_ROOT, relativePath: "src/a.ts", line: 12 },
    {
      candidate: "src/a.ts:12:3",
      root: POSIX_ROOT,
      relativePath: "src/a.ts",
      line: 12,
      column: 3,
    },
    { candidate: "C:\\repo\\a.ts:12", root: "C:\\repo", relativePath: "a.ts", line: 12 },
  ])(
    "strips the line suffix of $candidate and keeps it in the data",
    ({ candidate, root, relativePath, line, column }) => {
      expect(parseChatFilePath(candidate, root)).toEqual({ relativePath, line, column });
    },
  );

  it.each([
    { candidate: "src/a.ts.", expected: { relativePath: "src/a.ts" } },
    { candidate: "src/a.ts,", expected: { relativePath: "src/a.ts" } },
    { candidate: "src/a.ts)", expected: { relativePath: "src/a.ts" } },
    // The sentence dot rides behind a line suffix: the location survives it.
    { candidate: "src/a.ts:12.", expected: { relativePath: "src/a.ts", line: 12 } },
  ])("strips trailing sentence punctuation from $candidate", ({ candidate, expected }) => {
    expect(parseChatFilePath(candidate, POSIX_ROOT)).toEqual(expected);
  });
});

describe("workspace roots with trailing separators", () => {
  it.each([
    { candidate: "/home/u/repo/src/a.ts", root: "/home/u/repo/", relativePath: "src/a.ts" },
    { candidate: "/src/a.ts", root: "/", relativePath: "src/a.ts" },
    { candidate: "C:\\repo\\src\\a.ts", root: "C:\\repo\\", relativePath: "src/a.ts" },
    { candidate: "C:\\src\\a.ts", root: "C:\\", relativePath: "src/a.ts" },
  ])("resolves $candidate against $root", ({ candidate, root, relativePath }) => {
    expect(parseChatFilePath(candidate, root)).toEqual({ relativePath });
    expect(scanChatFilePaths(candidate, root).map((token) => token.link)).toEqual([
      { relativePath },
    ]);
  });
});

describe("file-shaped final segments", () => {
  it.each([
    { candidate: "text/html", links: false },
    { candidate: "image/png", links: false },
    { candidate: "application/json", links: false },
    { candidate: "feat/x", links: false },
    { candidate: "feat/.x", links: true },
    { candidate: "dir/..ts", links: true },
    { candidate: "a/.", links: false },
    { candidate: "a/..", links: false },
    { candidate: "origin/main", links: false },
    { candidate: "v1.2.3/4.5.6", links: false },
    { candidate: "1/2.5", links: false },
    { candidate: "docs/index.html#a/b", links: false },
    { candidate: "src/a.ts", links: true },
    { candidate: "src/.env", links: true },
    { candidate: "x/.gitignore", links: true },
    { candidate: "config/.npmrc", links: true },
    { candidate: "crates/x/Cargo.toml", links: true },
    { candidate: "dir/LICENSE", links: true },
    { candidate: "dir/VERSION", links: true },
    { candidate: "dir/.gitattributes", links: true },
    { candidate: "dir/.oracleignore", links: true },
    { candidate: "dir/LICENSE.refero_skill", links: true },
    { candidate: "dir/NOTICE", links: true },
    { candidate: "dir/RECORD", links: true },
    { candidate: "dir/WHEEL", links: true },
    { candidate: "dir/Makefile", links: true },
    { candidate: "dir/Dockerfile", links: true },
    { candidate: "dir/README", links: false },
    { candidate: "dir/a.abcdefghij", links: true },
    { candidate: "dir/a.abcdefghijk", links: false },
    { candidate: "dir/a.a1", links: true },
    { candidate: "dir/a.a_b", links: false },
    { candidate: "dir/123", links: false },
  ])("classifies $candidate as links=$links", ({ candidate, links }) => {
    const expected = links ? { relativePath: candidate } : null;
    expect(parseChatFilePath(candidate, POSIX_ROOT)).toEqual(expected);
    expect(scanChatFilePaths(candidate, POSIX_ROOT).map((token) => token.link)).toEqual(
      links ? [expected] : [],
    );
  });
});

describe("rejected candidates", () => {
  it.each([
    // Absolute but not inside the root: another folder, another drive, a
    // sibling that merely shares a prefix.
    { candidate: "/home/u/other/a.ts", root: POSIX_ROOT },
    { candidate: "/home/u/repo2/a.ts", root: POSIX_ROOT },
    { candidate: "/home/u/repo", root: POSIX_ROOT },
    { candidate: "/home/u/my.project", root: "/home/u/my.project" },
    { candidate: String.raw`C:\repo.ts`, root: String.raw`C:\repo.ts` },
    { candidate: "/home/u/REPO/src/a.ts", root: POSIX_ROOT },
    { candidate: "/home/U/repo/src/a.ts", root: POSIX_ROOT },
    { candidate: "C:/REPO2/src/a.ts", root: WINDOWS_ROOT },
    { candidate: "C:/rÉpo/src/a.ts", root: "C:/répo" },
    { candidate: "D:/repo/a.ts", root: WINDOWS_ROOT },
    { candidate: "C:/repo/a.ts", root: "/home/u/repo" },
    // Escapes, URLs, homes.
    { candidate: "../a.ts", root: POSIX_ROOT },
    { candidate: "a/../../x", root: POSIX_ROOT },
    { candidate: "https://x/y.ts", root: POSIX_ROOT },
    { candidate: "file:///C:/x/y.ts", root: WINDOWS_ROOT },
    { candidate: "~/x", root: POSIX_ROOT },
    { candidate: "~/dir/LICENSE", root: POSIX_ROOT },
    // No by-name lookup, no directories, nothing malformed.
    { candidate: "App.tsx", root: POSIX_ROOT },
    { candidate: "dir/", root: POSIX_ROOT },
    { candidate: "dir/a.ts/", root: POSIX_ROOT },
    { candidate: "a/..", root: POSIX_ROOT },
    { candidate: ".", root: POSIX_ROOT },
    { candidate: "src/a b.ts", root: POSIX_ROOT },
    { candidate: "src/a\0.ts", root: POSIX_ROOT },
    { candidate: "C:notdrive/x.ts", root: "C:/repo" },
    // A zero line or column is no location; the token stays text.
    { candidate: "src/x.ts:0", root: POSIX_ROOT },
    { candidate: "src/x.ts:1:0", root: POSIX_ROOT },
  ])("leaves $candidate plain", ({ candidate, root }) => {
    expect(parseChatFilePath(candidate, root)).toBeNull();
  });

  it("leaves an oversized token plain", () => {
    expect(parseChatFilePath(`a/${"b".repeat(1998)}`, POSIX_ROOT)).toBeNull();
  });

  it("rejects a file-shaped token beyond the length limit", () => {
    expect(parseChatFilePath(`a/${"b".repeat(1998)}.ts`, POSIX_ROOT)).toBeNull();
  });
});

describe("scanning a plain segment", () => {
  it("finds tokens and leaves the surrounding text and trailing punctuation outside them", () => {
    const text = "edit src/a.ts, then `x` b/../../y and src/b.ts:12:3.";
    const tokens = scanChatFilePaths(text, POSIX_ROOT);

    expect(tokens.map((token) => text.slice(token.start, token.end))).toEqual([
      "src/a.ts",
      "src/b.ts:12:3",
    ]);
    expect(tokens.map((token) => token.link)).toEqual([
      { relativePath: "src/a.ts" },
      { relativePath: "src/b.ts", line: 12, column: 3 },
    ]);
  });

  it("rejects a URL whole instead of leaving a path-shaped tail behind", () => {
    expect(scanChatFilePaths("see https://x/y.ts now", POSIX_ROOT)).toEqual([]);
  });

  it.each(["src/a\\.ts", "src/a\\.b.ts", "src/a\\_b.ts"])(
    "leaves an escaped path token plain: %s",
    (candidate) => {
      expect(scanChatFilePaths(candidate, POSIX_ROOT)).toEqual([]);
    },
  );

  it("leaves an oversized run plain without scanning it", () => {
    const text = `a/${"b".repeat(1998)}`;
    expect(scanChatFilePaths(text, POSIX_ROOT)).toEqual([]);
  });
});
