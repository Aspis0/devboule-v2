import { access, cp, mkdir, rm } from "node:fs/promises";
import { constants } from "node:fs";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The shared producer intentionally uses the package directory name as the id.
// Stage under "polis" so dist can remain Vite's conventional install output.
const execFileAsync = promisify(execFile);
const scriptDir = dirname(fileURLToPath(import.meta.url));
const packageRoot = resolve(scriptDir, "..");
const dist = resolve(packageRoot, "dist");
const stage = resolve(packageRoot, "polis");
const producer = resolve(packageRoot, "../../scripts/make-plugin-manifest.mjs");

await rm(stage, { recursive: true, force: true });
await mkdir(stage, { recursive: true });
await cp(dist, stage, { recursive: true });

const repoRoot = resolve(packageRoot, "../..");
const backendName = "polis-backend.exe";
let backendCopied = false;
for (const candidate of [
  resolve(repoRoot, "target/release", backendName),
  resolve(repoRoot, "target/debug", backendName),
]) {
  try {
    await access(candidate, constants.R_OK);
    await cp(candidate, resolve(stage, backendName));
    backendCopied = true;
    break;
  } catch {
    // The UI plugin is still installable without a backend binary; the host
    // will refuse plugin_invoke until this file is present and hashed.
  }
}

try {
  const producerArgs = [
    producer,
    stage,
    "--entry-ui",
    "index.html",
    "--name",
    "Polis",
    "--version",
    "0.1.0",
    "--capability",
    "oracle.search",
    "--capability",
    "workspace.root",
  ];
  if (backendCopied) {
    producerArgs.push("--entry-backend", backendName);
  }
  producerArgs.push("--write");
  await execFileAsync(process.execPath, producerArgs);
  await cp(resolve(stage, "plugin.json"), resolve(dist, "plugin.json"));
  if (backendCopied) {
    await cp(resolve(stage, backendName), resolve(dist, backendName));
  }
} finally {
  await rm(stage, { recursive: true, force: true });
}
