import assert from "node:assert/strict";
import { mkdir, mkdtemp, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { selectFreshestBackend } from "./write-plugin-manifest-selection.mjs";

test("selects the fresher backend instead of release merely winning by order", async () => {
  const directory = await mkdtemp(join(tmpdir(), "polis-manifest-"));
  const release = join(directory, "release", "polis-backend.exe");
  const debug = join(directory, "debug", "polis-backend.exe");
  await mkdir(join(directory, "release"));
  await mkdir(join(directory, "debug"));
  await writeFile(release, "old release");
  await writeFile(debug, "fresh debug");
  await utimes(release, new Date(1000), new Date(1000));
  await utimes(debug, new Date(2000), new Date(2000));

  assert.equal(await selectFreshestBackend([release, debug]), debug);
});

test("fails loudly when present candidates have the same mtime", async () => {
  const directory = await mkdtemp(join(tmpdir(), "polis-manifest-"));
  const release = join(directory, "release", "polis-backend.exe");
  const debug = join(directory, "debug", "polis-backend.exe");
  await mkdir(join(directory, "release"));
  await mkdir(join(directory, "debug"));
  await writeFile(release, "release");
  await writeFile(debug, "debug");
  await utimes(release, new Date(3000), new Date(3000));
  await utimes(debug, new Date(3000), new Date(3000));

  await assert.rejects(
    selectFreshestBackend([release, debug]),
    /ambiguous backend candidates/,
  );
});
