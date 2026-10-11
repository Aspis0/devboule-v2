import { describe, expect, it } from "vitest";
import { localPathFromHref } from "./markdownLocalPath";

describe("localPathFromHref", () => {
  it("keeps drive, home, POSIX and relative spellings as the path they name", () => {
    expect(localPathFromHref("C:/Users/gualt/kalsa/index.html")).toBe(
      "C:/Users/gualt/kalsa/index.html",
    );
    expect(localPathFromHref("C:\\Users\\gualt\\site.css")).toBe("C:\\Users\\gualt\\site.css");
    expect(localPathFromHref("~/notes/todo.md")).toBe("~/notes/todo.md");
    expect(localPathFromHref("/home/u/x.ts")).toBe("/home/u/x.ts");
    expect(localPathFromHref("src/app.ts")).toBe("src/app.ts");
  });

  it("strips a file:// scheme, and the slash before a drive letter", () => {
    expect(localPathFromHref("file:///C:/Users/gualt/x.html")).toBe("C:/Users/gualt/x.html");
  });

  it("unwraps an angle-bracket destination and decodes its percent escapes", () => {
    expect(localPathFromHref("<C:/with space/x.md>")).toBe("C:/with space/x.md");
    expect(localPathFromHref("C:/a%20b/x.md")).toBe("C:/a b/x.md");
  });

  it("removes a line fragment, leaving the path", () => {
    expect(localPathFromHref("src/app.ts#L12")).toBe("src/app.ts");
    expect(localPathFromHref("src/app.ts#L12C3-L20")).toBe("src/app.ts");
  });

  it("keeps a file URL that names another machine as its UNC spelling, never a local path", () => {
    expect(localPathFromHref("file://server/share/secrets.txt")).toBe("//server/share/secrets.txt");
  });

  it("returns null for a web or mail destination, which the external link handles", () => {
    expect(localPathFromHref("https://example.com/x")).toBeNull();
    expect(localPathFromHref("mailto:person@example.com")).toBeNull();
  });

  it("returns null for a destination with a control character, so no scheme hides in it", () => {
    expect(localPathFromHref("\u0001javascript:alert(1)")).toBeNull();
  });

  it("returns null for an empty destination", () => {
    expect(localPathFromHref("   ")).toBeNull();
    expect(localPathFromHref("<>")).toBeNull();
  });
});
