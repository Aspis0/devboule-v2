import { describe, expect, it } from "vitest";
import { workspaceRelativePath } from "./workspaceRelativePath";

const ROOT = "C:/Users/gualt/Desktop/test";

describe("workspaceRelativePath: inside the root", () => {
  it.each([
    ["C:/Users/gualt/Desktop/test/kalsa-site/index.html", "kalsa-site/index.html"],
    ["C:\\Users\\gualt\\Desktop\\test\\site.css", "site.css"],
    ["c:/users/GUALT/desktop/TEST/a.md", "a.md"],
    ["C:/Users/gualt/Desktop/test/src/../docs/x.md", "docs/x.md"],
    ["src/app.ts", "src/app.ts"],
    ["./src/app.ts", "src/app.ts"],
    ["C:/Users/gualt/Desktop/test/src/app.ts:12:3", "src/app.ts"],
  ])("maps %s to %s", (path, relative) => {
    expect(workspaceRelativePath(path, ROOT)).toBe(relative);
  });
});

describe("workspaceRelativePath: outside the root", () => {
  it.each([
    "C:/Windows/System32/drivers/etc/secrets.txt",
    "C:/Users/gualt/Desktop/test2/secrets.txt",
    "C:/Users/gualt/Desktop/test/../secrets.txt",
    "C:/Users/gualt/Desktop/test/../../secrets.txt",
    "D:/Users/gualt/Desktop/test/a.md",
    "src/../../secrets.txt",
    "../secrets.txt",
    "~/AppData/secret.txt",
    "~/Desktop/test/a.md",
    "/etc/passwd",
    "//server/share/secrets.txt",
    "C:folder/a.txt",
    "C:/Users/gualt/Desktop/test",
  ])("refuses %s", (path) => {
    expect(workspaceRelativePath(path, ROOT)).toBeNull();
  });

  it("refuses everything when the root is a UNC share", () => {
    expect(workspaceRelativePath("src/app.ts", "//server/share")).toBeNull();
  });
});

describe("workspaceRelativePath: a Windows verbatim drive path", () => {
  it.each([
    [
      String.raw`\\?\C:\Users\u\New folder\repo\src\a.ts`,
      String.raw`C:\Users\u\New folder\repo`,
      "src/a.ts",
    ],
    [
      String.raw`C:\Users\u\New folder\repo\src\a.ts`,
      String.raw`\\?\C:\Users\u\New folder\repo`,
      "src/a.ts",
    ],
    [
      String.raw`\\?\C:\Users\u\New folder\repo\src\a.ts`,
      String.raw`\\?\C:\Users\u\New folder\repo`,
      "src/a.ts",
    ],
  ])("maps %s inside the root %s", (path, root, relative) => {
    expect(workspaceRelativePath(path, root)).toBe(relative);
  });

  it("still refuses a verbatim UNC share", () => {
    expect(workspaceRelativePath(String.raw`\\?\UNC\server\share\a.ts`, ROOT)).toBeNull();
  });
});

describe("workspaceRelativePath: a POSIX root", () => {
  const POSIX_ROOT = "/home/u/project";

  it.each([
    ["/home/u/project/src/app.ts", "src/app.ts"],
    ["src/app.ts", "src/app.ts"],
    ["/home/u/project/src/../docs/x.md", "docs/x.md"],
  ])("maps %s to %s", (path, relative) => {
    expect(workspaceRelativePath(path, POSIX_ROOT)).toBe(relative);
  });

  it.each([
    "/home/u/project2/a.md",
    "/home/u/project/../secrets.txt",
    "/etc/passwd",
    "../secrets.txt",
    "C:/Users/gualt/a.md",
    "~/notes.md",
  ])("refuses %s", (path) => {
    expect(workspaceRelativePath(path, POSIX_ROOT)).toBeNull();
  });
});
