// @vitest-environment node

// The two halves of the browser controller are written in two languages and
// meet at a field name each of them serialises. Nothing at runtime compares
// them, so this reads the Rust source and checks the contract the TypeScript
// declares against it: a browser child webview is always an EXTERNAL page (a
// local URL would hand it the app's own IPC), every command the frontend calls
// is registered, and the app's capability grants no remote origin.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(import.meta.dirname, "../../..");
const read = (path: string): string => readFileSync(resolve(root, path), "utf8");

const rust = read("src-tauri/src/browser.rs");
const registry = read("src-tauri/src/browser/registry.rs");
const lib = read("src-tauri/src/lib.rs");
const capabilities = read("src-tauri/capabilities/default.json");
const controller = read("src/features/workspace/browserController.ts");

const CARGO = read("src-tauri/Cargo.toml");

describe("the Rust controller and the TypeScript door", () => {
  it("registers every command the frontend calls", () => {
    const invoked = [...controller.matchAll(/invoke(?:<[^>]*>)?\("(\w+)"/g)].map(
      (match) => match[1],
    );
    const registered = [...lib.matchAll(/browser::(browser_\w+)/g)].map((match) => match[1]);
    for (const command of invoked) {
      expect(registered, `${command} is invoked but not registered`).toContain(command);
    }
    // And nothing is registered that no caller reaches, which is how a
    // renamed command shows up instead of failing at runtime.
    for (const command of registered) {
      expect(invoked, `${command} is registered but never called`).toContain(command);
    }
  });

  it("gives every browser page an external URL, so no page is on the app's origin", () => {
    expect(rust).toContain("WebviewUrl::External");
    expect(rust).not.toContain("WebviewUrl::App(");
  });

  it("gates every navigation in the webview's own hook, not only the address bar", () => {
    expect(rust).toContain(".on_navigation(");
    expect(rust).toContain("url::gate(candidate)");
  });

  it("refuses a native popup and reports the request instead", () => {
    expect(rust).toContain("NewWindowResponse::Deny");
  });

  it("keeps the shared profile in one fixed folder this app owns", () => {
    expect(registry).toContain('const PROFILE_DIR: &str = "browser-profile";');
    expect(registry).toContain("app_local_data_dir.join(PROFILE_DIR)");
    // The profile is never derived from anything a page says.
    expect(registry).not.toMatch(/profile_dir\([^)]*(url|candidate|target)/i);
  });

  it("grants no capability to a remote origin, so a page cannot reach an app command", () => {
    const parsed: unknown = JSON.parse(capabilities);
    const list = parsed as { permissions?: unknown; remote?: unknown };
    expect(list.permissions).not.toContain("browser");
    expect(list.remote).toBeUndefined();
    // `core:default` alone would still be a grant if a remote block named it.
    expect(capabilities).not.toContain('"urls"');
  });

  it("enables the feature the child-webview API is behind", () => {
    expect(CARGO).toContain('"unstable"');
  });
});
