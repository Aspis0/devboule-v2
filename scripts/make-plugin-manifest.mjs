/**
 * Write a plugin's `plugin.json` from the directory it describes.
 *
 * The host refuses a plugin unless its manifest lists **every** file in the
 * directory with a matching SHA-256 (`src-tauri/src/plugins/manifest.rs`). That
 * is the right rule and it is also unusable by hand: a plugin with forty sprite
 * atlases means forty `Get-FileHash` invocations and a JSON object assembled
 * without a typo. A format with no producer is a format nobody can ship.
 *
 * So this walks the directory the way the verifier walks it — same exclusions,
 * same normalised paths, same refusals — and prints the manifest. Run it again
 * after changing a file; that is the whole workflow.
 *
 *   node scripts/make-plugin-manifest.mjs <directory> --entry-ui ui/index.js \
 *     [--name Polis] [--version 0.1.0] [--entry-backend polis-backend.exe] \
 *     [--capability oracle.search] [--write]
 *
 * Without `--write` it prints to stdout, so it can be inspected before it
 * replaces anything.
 *
 * Exit codes: 0 written or printed, 1 the directory cannot produce a valid
 * manifest, 2 the arguments are wrong.
 */

import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { lstat, readdir, writeFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

const MANIFEST_FILE_NAME = "plugin.json";
const MANIFEST_VERSION = 1;
/** Mirrors MAX_PLUGIN_FILES in manifest.rs. Producing more than the host will
 *  verify wastes the author's time twice. */
const MAX_FILES = 10_000;

function usage(message) {
  process.stderr.write(`${message}\n\nSee the comment at the top of this file.\n`);
  process.exit(2);
}

function parseArguments(argv) {
  const options = { capabilities: [], write: false };
  let directory = null;
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    const take = () => {
      const value = argv[index + 1];
      if (value === undefined) usage(`${argument} needs a value`);
      index += 1;
      return value;
    };
    switch (argument) {
      case "--name":
        options.name = take();
        break;
      case "--version":
        options.version = take();
        break;
      case "--entry-ui":
        options.ui = take();
        break;
      case "--entry-backend":
        options.backend = take();
        break;
      case "--capability":
        options.capabilities.push(take());
        break;
      case "--write":
        options.write = true;
        break;
      default:
        if (argument.startsWith("--")) usage(`unknown option ${argument}`);
        if (directory !== null) usage("only one directory can be described");
        directory = argument;
    }
  }
  if (directory === null) usage("no plugin directory given");
  if (!options.ui) usage("--entry-ui is required: the host will not load a plugin without one");
  return { directory: resolve(directory), options };
}

function hasControlCharacter(value) {
  for (const character of value) {
    const code = character.codePointAt(0);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}

/**
 * Every file under `root`, as the host will see it: forward slashes, the
 * top-level manifest left out, and links refused rather than followed.
 *
 * The refusals are copied deliberately. A producer that happily emits something
 * the verifier rejects moves the failure to the user's machine.
 */
async function listFiles(root, prefix = "") {
  const found = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    // `readdir` reports the entry itself, not its target, so this sees a link.
    if (entry.isSymbolicLink()) {
      throw new Error(`${relative} is a link, and a plugin has to be the files themselves`);
    }
    if (entry.isDirectory()) {
      found.push(...(await listFiles(join(root, entry.name), relative)));
      continue;
    }
    if (!entry.isFile()) throw new Error(`${relative} is neither a file nor a directory`);
    if (relative === MANIFEST_FILE_NAME) continue;
    if (relative.includes("\\") || relative.includes(":") || hasControlCharacter(relative)) {
      throw new Error(`${relative} cannot be addressed by the plugin server and must be renamed`);
    }
    if (relative.endsWith(".") || relative.endsWith(" ")) {
      // Windows strips these, so the name on disk and the name in the manifest
      // would stop agreeing.
      throw new Error(`${relative} ends in a dot or a space and must be renamed`);
    }
    found.push(relative);
  }
  return found;
}

function sha256(path) {
  return new Promise((resolveHash, rejectHash) => {
    const hash = createHash("sha256");
    createReadStream(path)
      .on("error", rejectHash)
      .on("data", (chunk) => hash.update(chunk))
      .on("end", () => resolveHash(hash.digest("hex")));
  });
}

async function main() {
  const { directory, options } = parseArguments(process.argv.slice(2));
  const stats = await lstat(directory).catch(() => null);
  if (!stats?.isDirectory()) {
    process.stderr.write(`${directory} is not a directory\n`);
    process.exit(1);
  }

  // The host requires the id to be the directory name, and the directory name
  // to be usable as a URL path segment. Saying so here beats shipping a plugin
  // that is refused on the user's machine for the name of its folder.
  const id = basename(directory);
  if (!/^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/.test(id) || id.length > 64) {
    process.stderr.write(
      `the directory is named "${id}", and a plugin id must be 1 to 64 characters of ` +
        "lowercase letters, digits and dashes, not starting or ending with a dash\n",
    );
    process.exit(1);
  }

  let relatives;
  try {
    relatives = (await listFiles(directory)).sort();
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exit(1);
  }
  if (relatives.length === 0) {
    process.stderr.write("the directory holds no files, so there would be nothing to verify\n");
    process.exit(1);
  }
  if (relatives.length > MAX_FILES) {
    process.stderr.write(`${relatives.length} files is more than the host will verify\n`);
    process.exit(1);
  }
  for (const entry of [options.ui, options.backend].filter(Boolean)) {
    if (!relatives.includes(entry)) {
      process.stderr.write(`${entry} is not in the directory, so it would run unverified\n`);
      process.exit(1);
    }
  }

  const files = {};
  for (const relative of relatives) {
    files[relative] = await sha256(join(directory, relative));
  }

  const manifest = {
    manifestVersion: MANIFEST_VERSION,
    id,
    name: options.name ?? id,
    version: options.version ?? "0.0.0",
    entry: options.backend ? { ui: options.ui, backend: options.backend } : { ui: options.ui },
    capabilities: [...new Set(options.capabilities)],
    files,
  };
  const text = `${JSON.stringify(manifest, null, 2)}\n`;
  if (options.write) {
    await writeFile(join(directory, MANIFEST_FILE_NAME), text);
    process.stderr.write(
      `wrote ${join(directory, MANIFEST_FILE_NAME)} (${relatives.length} files)\n`,
    );
  } else {
    process.stdout.write(text);
  }
}

await main();
