import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("./tauri", () => ({ externalUrlOpen: vi.fn(async () => undefined) }));

import { externalUrlOpen } from "./tauri";
import { openInBrowser } from "./openInBrowser";

afterEach(() => {
  vi.mocked(externalUrlOpen).mockClear();
});

describe("openInBrowser", () => {
  it("hands an http(s) URL to the shell command", () => {
    openInBrowser("https://example.com/a");
    openInBrowser("http://example.com/b");
    expect(vi.mocked(externalUrlOpen).mock.calls).toEqual([
      ["https://example.com/a"],
      ["http://example.com/b"],
    ]);
  });

  it("refuses a scheme the system browser must not receive", () => {
    for (const url of [
      "javascript:alert(1)",
      "data:text/html,<script>alert(1)</script>",
      "file:///etc/passwd",
      "mailto:someone@example.com",
    ]) {
      openInBrowser(url);
    }
    expect(vi.mocked(externalUrlOpen)).not.toHaveBeenCalled();
  });

  it("refuses a URL that carries credentials, the way the command does", () => {
    openInBrowser("https://user@example.com/a");
    openInBrowser("https://user:pass@example.com/a");
    expect(vi.mocked(externalUrlOpen)).not.toHaveBeenCalled();
  });

  it("still opens an at sign that is not userinfo", () => {
    openInBrowser("https://example.com/a@b");
    openInBrowser("https://example.com/?mail=user@example.com");
    expect(vi.mocked(externalUrlOpen).mock.calls).toEqual([
      ["https://example.com/a@b"],
      ["https://example.com/?mail=user@example.com"],
    ]);
  });

  it("answers a refused launch without a rejection", async () => {
    vi.mocked(externalUrlOpen).mockRejectedValueOnce(new Error("no browser"));
    openInBrowser("https://example.com/a");
    await Promise.resolve();
    expect(vi.mocked(externalUrlOpen)).toHaveBeenCalledTimes(1);
  });
});
