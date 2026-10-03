import { describe, expect, it } from "vitest";
import {
  BROWSER_START_URL,
  browserTabLabel,
  browserUrlRefusal,
  normalizeBrowserUrl,
} from "./browserUrl";

describe("normalizeBrowserUrl", () => {
  it("keeps a full http(s) address", () => {
    expect(normalizeBrowserUrl("https://example.com/page?q=1")).toBe(
      "https://example.com/page?q=1",
    );
    expect(normalizeBrowserUrl("http://127.0.0.1:1420/")).toBe("http://127.0.0.1:1420/");
  });

  it("gives schemeless text http:// for a loopback and https:// otherwise", () => {
    expect(normalizeBrowserUrl("example.com")).toBe("https://example.com/");
    expect(normalizeBrowserUrl("localhost:5173")).toBe("http://localhost:5173/");
    expect(normalizeBrowserUrl("127.0.0.1:8080/admin")).toBe("http://127.0.0.1:8080/admin");
    expect(normalizeBrowserUrl("[::1]:3000")).toBe("http://[::1]:3000/");
  });

  it("reads a host:port as a host and a port, not as a scheme", () => {
    expect(normalizeBrowserUrl("example.com:8443/status")).toBe("https://example.com:8443/status");
    expect(normalizeBrowserUrl("https://example.com:8443/")).toBe("https://example.com:8443/");
  });

  it("answers empty text with the start page", () => {
    expect(normalizeBrowserUrl("")).toBe(BROWSER_START_URL);
    expect(normalizeBrowserUrl("   ")).toBe(BROWSER_START_URL);
  });

  it("refuses file:, javascript: and custom schemes", () => {
    for (const raw of [
      "file:///C:/Windows/System32/drivers/etc/hosts",
      "javascript:alert(1)",
      "data:text/html,<script>1</script>",
      "tauri://localhost",
      "about:blank",
      "ftp://example.com",
    ]) {
      expect(normalizeBrowserUrl(raw)).toBeNull();
    }
  });

  it("refuses text that is not an address at all", () => {
    expect(normalizeBrowserUrl("how do i boil an egg")).toBeNull();
    expect(normalizeBrowserUrl("https://")).toBeNull();
  });

  it("reads the scheme off the parse, not off the text", () => {
    expect(normalizeBrowserUrl("HTTPS://Example.COM/Path")).toBe("https://example.com/Path");
    expect(normalizeBrowserUrl("  javascript:alert(1)  ")).toBeNull();
  });
});

describe("browserUrlRefusal", () => {
  it("says nothing for text that navigates", () => {
    expect(browserUrlRefusal("example.com")).toBeNull();
    expect(browserUrlRefusal("")).toBeNull();
  });

  it("names the scheme it refused", () => {
    expect(browserUrlRefusal("javascript:alert(1)")).toBe(
      "javascript: addresses cannot be opened here.",
    );
    expect(browserUrlRefusal("file:///etc/passwd")).toBe("file: addresses cannot be opened here.");
  });

  it("says the text is not an address when there is no scheme to name", () => {
    expect(browserUrlRefusal("two eggs")).toBe("That is not a web address.");
  });
});

describe("browserTabLabel", () => {
  it("prefers the page's own title", () => {
    expect(browserTabLabel("Example Domain", "https://example.com/")).toBe("Example Domain");
  });

  it("falls back to the hostname, then to the raw url", () => {
    expect(browserTabLabel(null, "https://example.com/deep/path")).toBe("example.com");
    expect(browserTabLabel("   ", "https://example.com/")).toBe("example.com");
    expect(browserTabLabel(null, "not a url")).toBe("not a url");
  });
});
