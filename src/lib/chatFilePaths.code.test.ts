import { describe, expect, it } from "vitest";
import { parseChatCodeFilePath, parseChatFilePath, scanChatFilePaths } from "./chatFilePaths";

const ROOT = String.raw`\\?\C:\Users\u\New folder\repo`;
const ABSOLUTE = String.raw`C:\Users\u\New folder\repo\src\a.ts`;
const VERBATIM_ABSOLUTE = ROOT + String.raw`\src\a.ts`;

describe("code-span file paths", () => {
  it.each([
    { candidate: "src/x.ts", relativePath: "src/x.ts" },
    { candidate: "src/x.ts:12", relativePath: "src/x.ts", line: 12 },
    { candidate: "src/x.ts:12:3", relativePath: "src/x.ts", line: 12, column: 3 },
    { candidate: "src/x.ts:012:003", relativePath: "src/x.ts", line: 12, column: 3 },
    { candidate: "src/New folder/a.ts", relativePath: "src/New folder/a.ts" },
    { candidate: "dir/..ts", relativePath: "dir/..ts" },
    { candidate: "src/folder. /a.ts", relativePath: "src/folder. /a.ts" },
    { candidate: ABSOLUTE, relativePath: "src/a.ts" },
    { candidate: `${ABSOLUTE}:12:3`, relativePath: "src/a.ts", line: 12, column: 3 },
    { candidate: VERBATIM_ABSOLUTE, relativePath: "src/a.ts" },
    // Outside the root, and `~`, stay clickable in their own spelling:
    // the File tab routes them app-only, never joined to the root.
    {
      candidate: String.raw`C:\Users\u\New folder\other\src\a.ts`,
      relativePath: "C:/Users/u/New folder/other/src/a.ts",
    },
    {
      candidate: String.raw`D:\Users\u\New folder\repo\src\a.ts`,
      relativePath: "D:/Users/u/New folder/repo/src/a.ts",
    },
    { candidate: "~/dir/LICENSE", relativePath: "~/dir/LICENSE" },
  ])("resolves the whole code span $candidate", ({ candidate, ...expected }) => {
    // Subset: the absolute opener form is pinned in chatFilePaths.test.ts.
    expect(parseChatCodeFilePath(candidate, ROOT)).toMatchObject(expected);
  });

  it.each([
    "src/a\\.ts",
    "src/a\\_b.ts",
    "./src/a.ts:12",
    "src/x/../a.ts:12",
    "src//a.ts:12",
    "src\\a.ts:12",
    "src/a.ts.",
    "src/a.ts:12.",
    "src/a.ts:0",
    "src/a.ts:1:0",
    "src/a.ts:1234567890",
    "src/a.ts:12:3:4",
    " src/a.ts",
    "src/a.ts ",
    "src/a.ts :12",
    "src/a\tb.ts",
    "src/a\nb.ts",
    "src/a\rb.ts",
    "src/a\0b.ts",
    "src/a\x1bb.ts",
    "src/a\x7fb.ts",
    "src/a\x85b.ts",
    "src/a\u00a0b.ts",
    "src/a\u2003b.ts",
    "../a.ts",
    "a/../../x.ts",
    "dir/.. /.. /a.ts",
    "https://x/y.ts",
    "file:///C:/repo/a.ts",
    "dir/a.ts/",
    "App.tsx",
    "text/html",
    "docs/index.html#a/b",
  ])("leaves malformed or altered code paths plain: %s", (candidate) => {
    expect(parseChatCodeFilePath(candidate, ROOT)).toBeNull();
  });

  it.each([
    String.raw`C:\Users\u\New folder\repo\src\..\a.ts`,
    String.raw`C:\Users\u\New folder\repo\src\\a.ts`,
    String.raw`\\?\C:\Users\u\New folder\repo\..\other\a.ts`,
    String.raw`C:\Users\u\New folder\REPO\src\a.ts`,
    ROOT,
  ])("rejects normalized absolute code paths: %s", (candidate) => {
    expect(parseChatCodeFilePath(candidate, ROOT)).toBeNull();
  });

  it("retains the candidate length bound with spaces enabled", () => {
    expect(parseChatCodeFilePath(`a/${"b ".repeat(600)}c.ts`, ROOT)).toBeNull();
  });
});

describe("dot and space segments", () => {
  it.each([".. ", ". ", "...", " ", ".. .", " . . "])(
    "refuses the ambiguous segment %s across platforms",
    (segment) => {
      const candidate = `dir/${segment}/a.ts`;
      expect(parseChatCodeFilePath(candidate, ROOT)).toBeNull();
      expect(parseChatCodeFilePath(candidate, "/home/u/repo")).toBeNull();
      expect(parseChatFilePath(candidate, ROOT)).toBeNull();
    },
  );
});

describe("drive-letter verbatim syntax", () => {
  it.each([
    { candidate: String.raw`\\?\C:\repo\src\a.ts`, root: "C:/repo" },
    { candidate: "C:/repo/src/a.ts", root: String.raw`\\?\C:\repo` },
    { candidate: String.raw`\\?\c:\repo\src\a.ts`, root: String.raw`\\?\C:\repo` + "\\" },
    { candidate: String.raw`c:\repo\Src\A.ts`, root: "C:/repo", relativePath: "Src/A.ts" },
  ])("canonicalizes candidate $candidate and root $root", ({ candidate, root, relativePath }) => {
    const expected = { relativePath: relativePath ?? "src/a.ts" };
    expect(parseChatFilePath(candidate, root)).toMatchObject(expected);
    expect(parseChatCodeFilePath(candidate, root)).toMatchObject(expected);
  });

  it.each([
    String.raw`\\server\share\a.ts`,
    "//server/share/a.ts",
    String.raw`\\?\UNC\server\share\a.ts`,
    String.raw`\\.\C:\repo\a.ts`,
    String.raw`\\?\C:repo\a.ts`,
  ])("rejects network and device candidates even under a POSIX root: %s", (candidate) => {
    expect(parseChatFilePath(candidate, "/")).toBeNull();
    expect(parseChatCodeFilePath(candidate, "/")).toBeNull();
  });

  it("rejects a UNC root for absolute candidates", () => {
    expect(parseChatCodeFilePath("/server/share/a.ts", String.raw`\\server\share`)).toBeNull();
  });
});

describe("spaces in prose", () => {
  it.each([ABSOLUTE, VERBATIM_ABSOLUTE])(
    "keeps the spaced absolute path plain: %s",
    (candidate) => {
      expect(parseChatFilePath(candidate, ROOT)).toBeNull();
      expect(scanChatFilePaths(`edit ${candidate} now`, ROOT)).toEqual([]);
    },
  );

  it("rejects a spaced POSIX absolute path without hiding a later separate file", () => {
    const text = "edit /home/u/New folder/repo/src/a.ts then src/b.ts";
    expect(
      scanChatFilePaths(text, "/home/u/New folder/repo").map((token) => token.link),
    ).toMatchObject([{ relativePath: "src/b.ts" }]);
  });

  it("rejects an absolute path with several spaces as a whole", () => {
    expect(
      scanChatFilePaths(String.raw`C:\Users\u\New very long folder\repo\src\a.ts`, ROOT),
    ).toEqual([]);
  });
});
