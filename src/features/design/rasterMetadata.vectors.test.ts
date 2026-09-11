// @vitest-environment node

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { RasterMimeType } from "./designAttachments";
import {
  RASTER_METADATA_RULES,
  type RasterMetadataRule,
  stripRasterMetadata,
} from "./rasterMetadata";

/**
 * The vectors are the shared evidence between this implementation and the Rust
 * one that is about to be written in the daemon, and they belong to neither:
 * they live at the repo root, outside both trees, because a fixture inside
 * either side's source directory reads as that side's property and gets edited
 * to match that side's behaviour.
 *
 * That makes this file's job smaller than it looks. It does not decide what the
 * right answer is — `rasterMetadata.test.ts` does that, in hand-written
 * assertions that read like sentences about the rule. This file checks that the
 * shared artefact is intact (not silently emptied, not missing a rule, not
 * carrying a message on a failure) and that the implementation agrees with it
 * byte for byte.
 */

interface PassingVector {
  name: string;
  mime: RasterMimeType;
  input: string;
  expect: { ok: true; output: string; removed: RasterMetadataRule[] };
}

interface RefusedVector {
  name: string;
  mime: RasterMimeType;
  input: string;
  expect: { ok: false };
}

type Vector = PassingVector | RefusedVector;

interface VectorsFile {
  note: string;
  vectors: Vector[];
}

const VECTORS_PATH = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "..",
  "fixtures",
  "raster-metadata",
  "vectors.json",
);

const vectorsFile = JSON.parse(readFileSync(VECTORS_PATH, "utf8")) as VectorsFile;

const RULE_NAMES = Object.keys(RASTER_METADATA_RULES) as RasterMetadataRule[];

/**
 * Lowercase hex, no separators. The check is not decoration: a vector written in
 * base64 or in `0x` form would still parse here and then differ from what the
 * Rust side reads, which is the one failure this whole arrangement exists to
 * make impossible.
 */
function decodeHex(hex: string): Uint8Array<ArrayBuffer> {
  if (hex.length === 0) return new Uint8Array(0);
  if (hex.length % 2 !== 0 || !/^[0-9a-f]+$/u.test(hex)) {
    throw new Error(`not lowercase hex with a whole number of bytes: "${hex.slice(0, 48)}"`);
  }
  const bytes = new Uint8Array(hex.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return bytes;
}

describe("the shared vectors file", () => {
  it("is not empty", () => {
    // A file that lost half its entries would otherwise pass every test below
    // silently, which is the failure mode the file exists to prevent.
    expect(vectorsFile.vectors.length).toBeGreaterThan(0);
  });

  it("names each vector once", () => {
    const names = vectorsFile.vectors.map((vector) => vector.name);
    expect(new Set(names).size).toBe(names.length);
  });

  it("covers both containers", () => {
    const mimes = new Set(vectorsFile.vectors.map((vector) => vector.mime));
    expect([...mimes].sort()).toEqual(["image/jpeg", "image/png"]);
  });

  it("covers every rule in RASTER_METADATA_RULES, and invents none", () => {
    // Equality rather than containment, in both directions on purpose. A new
    // rule added to the table without a vector fails here — the Rust side has to
    // learn the new vocabulary, not just the bytes it already agreed on — and a
    // vector naming a rule that no longer exists fails here too.
    const covered = new Set(
      vectorsFile.vectors.flatMap((vector) => (vector.expect.ok ? vector.expect.removed : [])),
    );
    expect([...covered].sort()).toEqual([...RULE_NAMES].sort());
  });

  it("has a clean vector per container, whose output is its input", () => {
    for (const mime of ["image/jpeg", "image/png"] as const) {
      const clean = vectorsFile.vectors.filter(
        (vector) => vector.mime === mime && vector.expect.ok && vector.expect.removed.length === 0,
      );
      expect(clean.length, mime).toBeGreaterThan(0);
      for (const vector of clean) {
        if (!vector.expect.ok) continue;
        expect(vector.expect.output, vector.name).toBe(vector.input);
      }
    }
  });

  it("has one refusal per container, and a refusal says nothing but ok", () => {
    for (const mime of ["image/jpeg", "image/png"] as const) {
      expect(
        vectorsFile.vectors.filter((vector) => vector.mime === mime && !vector.expect.ok).length,
        mime,
      ).toBeGreaterThan(0);
    }
    for (const vector of vectorsFile.vectors) {
      if (vector.expect.ok) continue;
      // No message, no reason, and no code: the two sides' failures have
      // different audiences — a sentence shown to the designer in the composer
      // against a WireError on the daemon's pipe — and pinning the wording would
      // force one of them to read the other's sentence. The rule is restated in
      // the file's own `note`.
      expect(Object.keys(vector.expect), vector.name).toEqual(["ok"]);
    }
  });

  it("writes its bytes as hex and says why a failure is bare in its note", () => {
    for (const vector of vectorsFile.vectors) {
      expect(() => decodeHex(vector.input), vector.name).not.toThrow();
      if (!vector.expect.ok) continue;
      // Bound to a local: the narrowing of a property path does not survive
      // inside the arrow the assertion needs.
      const output = vector.expect.output;
      expect(() => decodeHex(output), vector.name).not.toThrow();
    }
    expect(vectorsFile.note.length).toBeGreaterThan(0);
  });
});

describe("stripRasterMetadata against the shared vectors", () => {
  for (const vector of vectorsFile.vectors) {
    it(vector.name, () => {
      const input = decodeHex(vector.input);
      const result = stripRasterMetadata(input, vector.mime);

      if (!vector.expect.ok) {
        // A file this pass cannot walk is refused rather than handed back: the
        // bytes were never examined, so calling them clean would be a promise
        // nothing here can keep.
        expect(result.ok, `${vector.name}: expected a refusal`).toBe(false);
        return;
      }

      if (!result.ok)
        throw new Error(`${vector.name}: expected a stripped file, got: ${result.reason}`);

      // Sorted on this side only: the JSON carries a set, and the order the
      // composer's sentence puts them in is a separate promise, asserted in
      // rasterMetadata.test.ts.
      expect([...result.removed].sort(), `${vector.name}: rules fired`).toEqual(
        vector.expect.removed,
      );

      const expected = decodeHex(vector.expect.output);
      expect([...result.bytes], `${vector.name}: output bytes`).toEqual([...expected]);
      if (vector.expect.removed.length === 0) {
        // A clean vector is a file this pass may not rewrite at all, so the
        // assertion is against the input bytes rather than against `ok`.
        expect([...result.bytes], `${vector.name}: a clean file comes back unchanged`).toEqual([
          ...input,
        ]);
      }
    });
  }
});
