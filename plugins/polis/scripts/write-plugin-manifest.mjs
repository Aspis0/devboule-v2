import { cp, mkdir, rm } from "node:fs/promises";
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
try {
  await execFileAsync(process.execPath, [
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
    "--write",
  ]);
  await cp(resolve(stage, "plugin.json"), resolve(dist, "plugin.json"));
} finally {
  await rm(stage, { recursive: true, force: true });
}
