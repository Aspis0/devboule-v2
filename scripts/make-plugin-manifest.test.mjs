import { execFileSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pluginDirectory = join(repoRoot, "plugins", "hello");
const generator = join(repoRoot, "scripts", "make-plugin-manifest.mjs");

function filesUnder(root, directory = root) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...filesUnder(root, path));
    } else if (entry.isFile() && entry.name !== "plugin.json") {
      files.push(relative(root, path).replaceAll("\\", "/"));
    }
  }
  return files.sort();
}

describe("make-plugin-manifest", () => {
  it("lists every plugin file with a valid digest and HTML entry", () => {
    const output = execFileSync(
      process.execPath,
      [
        generator,
        pluginDirectory,
        "--entry-ui",
        "ui/index.html",
        "--name",
        "Hello",
        "--version",
        "0.1.0",
        "--capability",
        "oracle.search",
      ],
      { cwd: repoRoot, encoding: "utf8" },
    );
    const manifest = JSON.parse(output);

    expect(manifest.id).toBe("hello");
    expect(manifest.entry.ui).toBe("ui/index.html");
    expect(Object.keys(manifest.files).sort()).toEqual(filesUnder(pluginDirectory));
    expect(manifest.files[manifest.entry.ui]).toMatch(/^[0-9a-f]{64}$/);
    for (const digest of Object.values(manifest.files)) {
      expect(digest).toMatch(/^[0-9a-f]{64}$/);
    }
  });
});
