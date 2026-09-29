// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyTheme, GROUND_BY_THEME } from "./theme";
import { syncNativeWindowBackground } from "./nativeWindowBackground";

const invokeMock = vi.hoisted(() => vi.fn());

// The boundary under test is the IPC wire: the real window/webview modules
// run, and only core.invoke is replaced, so a wrong argument key fails here.
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const appRoot = resolve(import.meta.dirname, "../..");

/** tokens.css is the single source: every copy below must equal its grounds. */
function tokenGrounds(): { light: string; dark: string } {
  const css = readFileSync(resolve(appRoot, "src/styles/tokens.css"), "utf8");
  const fresh = css.slice(0, css.indexOf("Legacy aliases"));
  const light = /:root\s*\{([^}]*)\}/.exec(fresh)?.[1] ?? "";
  const dark = /\[data-theme="dark"\]\s*\{([^}]*)\}/.exec(fresh)?.[1] ?? "";
  const read = (block: string): string =>
    /--ground-app\s*:\s*([^;]+);/.exec(block)?.[1]?.trim().toLowerCase() ?? "";
  return { light: read(light), dark: read(dark) };
}

function asRustTriplet(hex: string): string {
  const bytes = [hex.slice(1, 3), hex.slice(3, 5), hex.slice(5, 7)].map(
    (part) => `0x${part.toUpperCase()}`,
  );
  return bytes.join(", ");
}

function asConfigHex(channels: readonly number[]): string {
  const [red, green, blue] = channels;
  return `#${[red, green, blue].map((channel) => channel!.toString(16).padStart(2, "0")).join("")}`;
}

/** Strips // and block comments, leaving double-quoted strings alone. */
function stripComments(source: string): string {
  let out = "";
  let i = 0;
  while (i < source.length) {
    const char = source[i];
    const next = source[i + 1];
    if (char === '"') {
      let end = i + 1;
      while (end < source.length && source[end] !== '"') {
        end += source[end] === "\\" ? 2 : 1;
      }
      out += source.slice(i, end + 1);
      i = end + 1;
      continue;
    }
    if (char === "/" && next === "/") {
      const end = source.indexOf("\n", i);
      i = end === -1 ? source.length : end;
      continue;
    }
    if (char === "/" && next === "*") {
      const end = source.indexOf("*/", i + 2);
      i = end === -1 ? source.length : end + 2;
      continue;
    }
    out += char;
    i += 1;
  }
  return out;
}

/** Block comments only: CSS has no line comments, and a url(http://…) must survive. */
function normalizedStyleBlock(style: string): string {
  return style
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/\s+/g, " ")
    .trim();
}

/** The balanced {…} block starting at openIndex, inclusive. */
function balancedBlock(source: string, openIndex: number): string {
  let depth = 0;
  for (let index = openIndex; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    else if (source[index] === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(openIndex, index + 1);
    }
  }
  throw new Error("unbalanced braces");
}

/** This file's JSON5 subset (// comments, trailing commas) as plain JSON. */
function parseConfigJson5(source: string): {
  app: { windows: { label?: string; backgroundColor?: number[] }[] };
} {
  const withoutComments = stripComments(source);
  const withoutTrailingCommas = withoutComments.replace(/,(?=\s*[}\]])/g, "");
  return JSON.parse(withoutTrailingCommas) as {
    app: { windows: { label?: string; backgroundColor?: number[] }[] };
  };
}

function underTauri(): void {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
    },
  };
}

function fakeMedia(dark: boolean): () => MediaQueryList {
  return () => ({ matches: dark }) as unknown as MediaQueryList;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  document.documentElement.style.removeProperty("--ground-app");
  document.documentElement.removeAttribute("data-theme");
  document.getElementById("native-background-probe")?.remove();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe("the ground copies", () => {
  it("index.html keys the pre-mount ground on data-theme, not on the OS", () => {
    const { light, dark } = tokenGrounds();
    const html = readFileSync(resolve(appRoot, "index.html"), "utf8");
    const head = html.slice(0, html.indexOf("</head>"));
    const raw = /<style>([\s\S]*?)<\/style>/.exec(head)?.[1] ?? "";
    expect(raw, "a pre-mount <style> rule must exist in <head>").not.toBe("");
    const style = normalizedStyleBlock(raw);
    expect(style).toContain(`html[data-theme="light"] { background-color: ${light}; }`);
    expect(style).toContain(`html[data-theme="dark"] { background-color: ${dark}; }`);
  });

  it("the OS query is only the themeless fallback, so a stored dark wins on a light OS", () => {
    const { light, dark } = tokenGrounds();
    const html = readFileSync(resolve(appRoot, "index.html"), "utf8");
    const raw = /<style>([\s\S]*?)<\/style>/.exec(html)?.[1] ?? "";
    const style = normalizedStyleBlock(raw);
    expect(style).toContain(`html:not([data-theme]) { background-color: ${light}; }`);
    const mediaAt = style.indexOf("@media");
    expect(mediaAt, "the OS fallback must exist").toBeGreaterThan(-1);
    const media = balancedBlock(style, style.indexOf("{", mediaAt));
    expect(media).toContain(`html:not([data-theme]) { background-color: ${dark}; }`);
    // The bootstrap always sets data-theme before the first paint, so the
    // query must never name a resolved theme: `[data-theme="` with a value
    // would let the OS outvote the stored choice it was written for.
    expect(media).not.toContain('[data-theme="');
    const outsideMedia = style.replace(media, "");
    expect(outsideMedia).toContain(`html[data-theme="dark"] { background-color: ${dark}; }`);
  });

  it("the theme-color meta starts on the light ground", () => {
    const { light } = tokenGrounds();
    const html = readFileSync(resolve(appRoot, "index.html"), "utf8");
    const head = html.slice(0, html.indexOf("</head>"));
    const content = /<meta name="theme-color" content="([^"]+)" \/>/.exec(head)?.[1];
    expect(content?.toLowerCase()).toBe(light);
  });

  it("tauri.conf.json starts the main window's native layers on the dark ground", () => {
    const { dark } = tokenGrounds();
    const config = parseConfigJson5(
      readFileSync(resolve(appRoot, "src-tauri/tauri.conf.json"), "utf8"),
    );
    const main = config.app.windows.find((window) => window.label === "main");
    const color = main?.backgroundColor;
    expect(color, "the main window needs a static backgroundColor").toBeDefined();
    // A static config cannot follow both themes; dark is the choice that can
    // never show the reported white, and setup corrects light systems before
    // the first presented frame.
    expect(asConfigHex(color!)).toBe(dark);
    expect(color![3], "the startup layer is opaque").toBe(255);
  });

  it("window_background.rs carries both grounds and paints from the OS theme", () => {
    const { light, dark } = tokenGrounds();
    const source = stripComments(
      readFileSync(resolve(appRoot, "src-tauri/src/window_background.rs"), "utf8"),
    );
    expect(source).toContain(asRustTriplet(light));
    expect(source).toContain(asRustTriplet(dark));
    expect(source).toContain("set_background_color");
    expect(source, "the choice is the OS theme, never a constant").toContain(".theme()");
  });

  it("is the first statement of the lib.rs setup body", () => {
    const lib = stripComments(readFileSync(resolve(appRoot, "src-tauri/src/lib.rs"), "utf8"));
    expect(lib).toContain("mod window_background;");
    // Presence and position only: a mock runtime cannot observe the paint, so
    // this canary — not a proof — pins that the call opens setup, ahead of
    // every concession that follows.
    const setupAt = lib.indexOf(".setup(");
    const body = balancedBlock(lib, lib.indexOf("{", setupAt));
    expect(
      body.replace(/^\{\s*/, "").startsWith("window_background::paint_startup_background(app);"),
    ).toBe(true);
  });

  it("the capability grant is exactly the two background setters", () => {
    const capabilities = JSON.parse(
      readFileSync(resolve(appRoot, "src-tauri/capabilities/default.json"), "utf8"),
    ) as { permissions: string[] };
    expect([...capabilities.permissions].sort()).toEqual(
      [
        "core:default",
        "core:window:allow-set-background-color",
        "core:webview:allow-set-webview-background-color",
        "dialog:default",
        "notification:default",
      ].sort(),
    );
  });
});

describe("the IPC wire", () => {
  it("sends value, not color, to both background commands", async () => {
    underTauri();
    document.documentElement.style.setProperty("--ground-app", GROUND_BY_THEME.dark);
    applyTheme("dark", fakeMedia(true));
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("plugin:window|set_background_color", {
        label: "main",
        value: GROUND_BY_THEME.dark,
      }),
    );
    expect(invokeMock).toHaveBeenCalledWith("plugin:webview|set_webview_background_color", {
      label: "main",
      value: GROUND_BY_THEME.dark,
    });
    // Exactly one paint: a double-fire would pass the assertions above.
    await sleep(30);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });
});

describe("applying a theme paints both native layers", () => {
  it("hands the computed ground to both commands, dark and light", async () => {
    underTauri();
    document.documentElement.style.setProperty("--ground-app", GROUND_BY_THEME.dark);
    applyTheme("dark", fakeMedia(true));
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("plugin:webview|set_webview_background_color", {
        label: "main",
        value: GROUND_BY_THEME.dark,
      }),
    );

    invokeMock.mockClear();
    document.documentElement.style.setProperty("--ground-app", GROUND_BY_THEME.light);
    applyTheme("light", fakeMedia(false));
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("plugin:webview|set_webview_background_color", {
        label: "main",
        value: GROUND_BY_THEME.light,
      }),
    );
  });

  it("reads the painted ground, not the mirror: a drifted token still matches the page", async () => {
    underTauri();
    const probe = document.createElement("style");
    probe.id = "native-background-probe";
    probe.textContent = '[data-theme="dark"] { --ground-app: #0a0b0c; }';
    document.head.appendChild(probe);
    // No inline property anywhere: the only --ground-app in the document is
    // the stylesheet's, resolved through data-theme — the real app path.
    applyTheme("dark", fakeMedia(true));
    // A mirror of the token would send the dark ground; the page shows this.
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("plugin:window|set_background_color", {
        label: "main",
        value: "#0a0b0c",
      }),
    );
  });

  it("reads the light twin through the same path", async () => {
    underTauri();
    const probe = document.createElement("style");
    probe.id = "native-background-probe";
    probe.textContent = '[data-theme="light"] { --ground-app: #0b0c0d; }';
    document.head.appendChild(probe);
    applyTheme("light", fakeMedia(false));
    await vi.waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("plugin:window|set_background_color", {
        label: "main",
        value: "#0b0c0d",
      }),
    );
  });

  it.each([
    ["#abc", true],
    ["#ABCDEF", true],
    ["#abcdef", true],
    ["#abcd", false],
    ["#abcdef12", false],
    ["#16120e00", false],
    ["rgb(1 2 3)", false],
    ["", false],
  ])("gatekeeps %s for the wire: accepted is %s", async (token, accepted) => {
    underTauri();
    invokeMock.mockClear();
    if (token === "") document.documentElement.style.removeProperty("--ground-app");
    else document.documentElement.style.setProperty("--ground-app", token);
    syncNativeWindowBackground();
    if (accepted) {
      await vi.waitFor(() =>
        expect(invokeMock).toHaveBeenCalledWith("plugin:window|set_background_color", {
          label: "main",
          value: token,
        }),
      );
    } else {
      await sleep(20);
      expect(invokeMock).not.toHaveBeenCalled();
    }
  });

  it("outside Tauri it neither calls nor throws, and says nothing", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    const error = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      document.documentElement.style.setProperty("--ground-app", GROUND_BY_THEME.dark);
      expect(() => applyTheme("dark", fakeMedia(true))).not.toThrow();
      await sleep(20);
      expect(invokeMock).not.toHaveBeenCalled();
      expect(warn).not.toHaveBeenCalled();
      expect(error).not.toHaveBeenCalled();
    } finally {
      warn.mockRestore();
      error.mockRestore();
    }
  });

  it("says each failure kind once, then keeps the startup colour", async () => {
    vi.resetModules();
    const fresh = await import("./nativeWindowBackground");
    underTauri();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      fresh.syncNativeWindowBackground();
      fresh.syncNativeWindowBackground();
      document.documentElement.style.setProperty(
        "--ground-app",
        "color-mix(in srgb, red 50%, blue)",
      );
      fresh.syncNativeWindowBackground();
      expect(warn).toHaveBeenCalledTimes(1);
      expect(String(warn.mock.calls[0]?.[0])).toContain("--ground-app");
      expect(invokeMock).not.toHaveBeenCalled();
      // The warn gates the message, not the call: a good token syncs after.
      document.documentElement.style.setProperty("--ground-app", GROUND_BY_THEME.dark);
      fresh.syncNativeWindowBackground();
      await vi.waitFor(() =>
        expect(invokeMock).toHaveBeenCalledWith("plugin:window|set_background_color", {
          label: "main",
          value: GROUND_BY_THEME.dark,
        }),
      );
      expect(warn).toHaveBeenCalledTimes(1);
    } finally {
      warn.mockRestore();
    }
  });

  it("a refused invoke warns once instead of failing silently", async () => {
    vi.resetModules();
    const fresh = await import("./nativeWindowBackground");
    underTauri();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);
    try {
      invokeMock.mockRejectedValue(new Error("denied"));
      document.documentElement.style.setProperty("--ground-app", GROUND_BY_THEME.dark);
      fresh.syncNativeWindowBackground();
      fresh.syncNativeWindowBackground();
      await vi.waitFor(() => expect(warn).toHaveBeenCalledTimes(1));
      expect(String(warn.mock.calls[0]?.[0])).toContain("refused");
      // Both syncs ran to completion: the refused window invoke skips its
      // chained webview invoke, so one call each — and one warn for the kind.
      await sleep(30);
      expect(invokeMock).toHaveBeenCalledTimes(2);
    } finally {
      warn.mockRestore();
    }
  });
});
