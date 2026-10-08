// Quoting invariant for the installer's beforeBuildCommand: the chain runs
// through `cmd /S /C`, where a double quote (Rust argv escapes it to \",
// which PowerShell then reads as a string literal instead of code) or an
// unquoted & (a cmd separator) turns the stage segment into a silent no-op
// that exits 0. The rule lives here so a careless edit fails the gate, not
// the next release.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const configPath = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
  "tauri.installer.conf.json",
);

// JSONC → JSON: drop // line comments outside string literals (the config's
// comments are line comments; strings may contain //, e.g. schema URLs).
function stripLineComments(text) {
  let out = "";
  let inString = false;
  let escaped = false;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (inString) {
      out += char;
      if (escaped) escaped = false;
      else if (char === "\\") escaped = true;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') {
      inString = true;
      out += char;
      continue;
    }
    if (char === "/" && text[index + 1] === "/") {
      while (index < text.length && text[index] !== "\n") index += 1;
      out += "\n";
      continue;
    }
    out += char;
  }
  return out;
}

function beforeBuildCommand() {
  const config = JSON.parse(stripLineComments(readFileSync(configPath, "utf8")));
  const command = config.build?.beforeBuildCommand;
  expect(typeof command).toBe("string");
  expect(command).toContain(" -Command ");
  return command;
}

describe("installer beforeBuildCommand quoting invariant", () => {
  it("holds no double quote anywhere in the chain", () => {
    expect(beforeBuildCommand().includes('"')).toBe(false);
  });

  it("holds no & outside the && chain operators", () => {
    const withoutChainOperators = beforeBuildCommand().replaceAll("&&", "");
    expect(withoutChainOperators.includes("&")).toBe(false);
  });
});
