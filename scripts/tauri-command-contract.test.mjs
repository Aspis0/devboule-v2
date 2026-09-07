import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = join(import.meta.dirname, "..");
const srcRoot = join(repoRoot, "src");
const tauriClientPath = join(srcRoot, "lib", "tauri.ts");
const backendPath = join(repoRoot, "src-tauri", "src", "lib.rs");

function sourceFiles(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...sourceFiles(path));
    } else if (entry.isFile() && /\.(ts|tsx)$/.test(entry.name)) {
      files.push(path);
    }
  }
  return files;
}

function isTestFile(path) {
  return /\.(test|spec)\.(ts|tsx)$/.test(path);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function frontendCommandMap() {
  const tauriClient = readFileSync(tauriClientPath, "utf8");
  const wrappers = new Map();
  const wrapperPattern = /export\s+const\s+(\w+)\s*=\s*[\s\S]*?invokeTyped\(\s*["']([^"']+)["']/g;
  for (const match of tauriClient.matchAll(wrapperPattern)) {
    wrappers.set(match[1], match[2]);
  }

  const commands = new Map();
  for (const path of sourceFiles(srcRoot)) {
    if (path === tauriClientPath || isTestFile(path)) continue;
    const source = readFileSync(path, "utf8");

    for (const match of source.matchAll(/\binvokeTyped\(\s*["']([^"']+)["']/g)) {
      commands.set(match[1], path);
    }

    const importsPattern = /import\s*{([\s\S]*?)}\s*from\s*["'][^"']*lib\/tauri["']/g;
    for (const match of source.matchAll(importsPattern)) {
      for (const rawSpecifier of match[1].split(",")) {
        const specifier = rawSpecifier.trim().replace(/^type\s+/, "");
        if (!specifier) continue;
        const [imported, local = imported] = specifier.split(/\s+as\s+/);
        const command = wrappers.get(imported);
        if (command === undefined) continue;
        const callPattern = new RegExp(`\\b${escapeRegExp(local)}\\s*\\(`);
        if (callPattern.test(source)) commands.set(command, path);
      }
    }
  }
  return commands;
}

function registeredBackendCommands() {
  const backend = readFileSync(backendPath, "utf8");
  const handler = backend.match(/tauri::generate_handler!\s*\[([\s\S]*?)\]\s*\)/);
  if (handler === null) throw new Error("Could not find the Tauri generate_handler! block");

  const commands = new Set();
  for (const match of handler[1].matchAll(/\b([A-Za-z_]\w*(?:::[A-Za-z_]\w+)*)\s*,/g)) {
    commands.add(match[1].split("::").at(-1));
  }
  return commands;
}

describe("frontend/backend Tauri command contract", () => {
  it("registers every command actually invoked by non-test frontend code", () => {
    // Check calls, not every wrapper declared in tauri.ts: unused declarations are
    // not defects, while an invoked command needs a registration in the backend.
    const invoked = frontendCommandMap();
    const registered = registeredBackendCommands();
    const missing = [...invoked.keys()].filter((command) => !registered.has(command));

    if (missing.length > 0) {
      const details = missing.map((command) => `${command} (${invoked.get(command)})`).join(", ");
      throw new Error(`Frontend invokes unregistered Tauri command(s): ${details}`);
    }

    expect(invoked.size).toBeGreaterThan(0);
  });
});
