import { describe, expect, it } from "vitest";
import { browserChrome, submitBrowserAddress, type BrowserPage } from "./browserChrome";
import { BROWSER_START_URL } from "./browserUrl";

const PAGE: BrowserPage = {
  url: "https://example.com/",
  loading: false,
  canGoBack: true,
  canGoForward: false,
  error: null,
};

describe("browserChrome", () => {
  it("is a reload button while the page is idle", () => {
    expect(browserChrome(PAGE, null).action).toBe("reload");
  });

  it("becomes stop while the page loads, and reloads again after", () => {
    expect(browserChrome({ ...PAGE, loading: true }, null).action).toBe("stop");
    expect(browserChrome({ ...PAGE, loading: false }, null).action).toBe("reload");
  });

  it("keeps the history buttons disabled while the page says it cannot go", () => {
    const chrome = browserChrome({ ...PAGE, canGoBack: false, canGoForward: false }, null);
    expect(chrome.canGoBack).toBe(false);
    expect(chrome.canGoForward).toBe(false);
    // Reload and Stop stay available even where the page can go nowhere:
    // neither is a history answer.
    expect(chrome.action).toBe("reload");
  });

  it("shows the page's own address when the user is not typing", () => {
    expect(browserChrome(PAGE, null).barValue).toBe("https://example.com/");
  });

  it("keeps showing what the user typed while they are typing it", () => {
    expect(browserChrome(PAGE, "example.org/pa").barValue).toBe("example.org/pa");
  });

  it("shows the inline error line the controller sent", () => {
    expect(
      browserChrome({ ...PAGE, error: "`file:` addresses cannot be opened here." }, null).error,
    ).toBe("`file:` addresses cannot be opened here.");
  });
});

describe("submitBrowserAddress", () => {
  it("normalises a typed address into the one it will load", () => {
    expect(submitBrowserAddress("example.org")).toEqual({
      url: "https://example.org/",
      error: null,
    });
  });

  it("answers empty text with the start page", () => {
    expect(submitBrowserAddress("")).toEqual({ url: BROWSER_START_URL, error: null });
  });

  it("refuses a scheme this tab will never load, and says which", () => {
    expect(submitBrowserAddress("javascript:alert(1)")).toEqual({
      url: null,
      error: "javascript: addresses cannot be opened here.",
    });
    expect(submitBrowserAddress("file:///etc/passwd").error).toBe(
      "file: addresses cannot be opened here.",
    );
  });

  it("refuses text that is not an address, without navigating to the page's own", () => {
    expect(submitBrowserAddress("two eggs")).toEqual({
      url: null,
      error: "That is not a web address.",
    });
  });
});
