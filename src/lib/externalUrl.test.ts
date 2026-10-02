import { describe, expect, it } from "vitest";
import { openableUrl, opensExternally } from "./externalUrl";

const PREFIX = "https://example.com/";
const CEILING = 8192;
// Two-byte characters: the ceiling is in bytes, so a character count would let
// the over-limit input through.
const AT_CEILING = `${PREFIX}${"é".repeat((CEILING - PREFIX.length) / 2)}`;

// Every row mirrors a case in src-tauri/src/backend/tests/open_external.rs: the
// page cancels a click only for a URL the command opens.
const OPENED = [
  "http://example.com/a",
  "https://example.com/a?q=1#f",
  "HTTPS://EXAMPLE.com/a",
  "https://example.com/a@b",
  "https://example.com/?mail=user@example.com",
  "https://example.com/#user@example.com",
  AT_CEILING,
];

const REFUSED = [
  ["over the byte ceiling", `${AT_CEILING}a`],
  ["ftp scheme", "ftp://example.com/a"],
  ["javascript scheme", "javascript:alert(1)"],
  ["file scheme", "file:///etc/passwd"],
  ["mailto scheme", "mailto:someone@example.com"],
  ["no scheme", "example.com/a"],
  ["empty", ""],
  ["user", "https://user@example.com/a"],
  ["user and password", "https://user:pass@example.com/a"],
  ["password only", "http://:pass@example.com/a"],
  ["user before an IPv6 host", "https://user@[::1]/"],
  ["empty userinfo", "https://@example.com/"],
  ["empty user and password", "https://:@example.com/"],
  ["leading space", " https://example.com/a"],
  ["trailing space", "https://example.com/a "],
  ["inner space", "https://example.com/a b"],
  ["tab", "https://example.com/a\tb"],
  ["newline", "https://example.com/a\nb"],
  ["NUL", "https://example.com/a\u0000b"],
  ["DEL", "https://example.com/a\u007fb"],
  ["no-break space", "https://example.com/a b"],
  ["next line", "https://example.com/a\u0085b"],
  ["ideographic space", "https://example.com/a　b"],
  ["unclosed IPv6 host", "https://[::1"],
  ["no host", "https://"],
] as const;

describe("opensExternally", () => {
  it.each(OPENED)("accepts %#: the command opens it", (url) => {
    expect(opensExternally(url)).toBe(true);
  });

  it.each(REFUSED)("refuses %s: the command refuses it", (_name, url) => {
    expect(opensExternally(url)).toBe(false);
  });
});

describe("openableUrl", () => {
  it("answers the parsed URL for one the command opens", () => {
    expect(openableUrl("https://münich.example/a")?.href).toBe("https://xn--mnich-kva.example/a");
  });

  it("answers null for one the command refuses", () => {
    expect(openableUrl("https://user@example.com/a")).toBeNull();
  });
});
