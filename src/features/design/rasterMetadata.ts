import type { RasterMimeType } from "./designAttachments";

/**
 * Taking identity out of a raster attachment before it is carried anywhere.
 *
 * A photograph is two files in one. One of them is pixels; the other is a set of
 * side channels a camera or an editor wrote next to them — the GPS fix where it
 * was taken (EXIF or XMP), the device model and its serial number, free-text
 * notes and a time of day. Nothing in the composer needs any of that to draw the
 * image, and an attachment now leaves the machine: the bytes travel to the
 * daemon, which writes them to a file the agent opens. This pass landed one
 * commit before that delivery did, and the order was the point — a strip added
 * afterwards cleans new imports and leaves every file already held dirty, and a
 * composer's attachments outlive the run they were attached for.
 *
 * The pass works on the container's own segments, never on a re-encode. Running
 * a raster through a canvas and reading `toDataURL` back would remove the
 * metadata and also lose a generation of image quality, need a DOM, and be
 * untestable outside a browser. Walking the byte structure touches only the
 * segments that hold identity and copies everything else — including the
 * entropy-coded scan, bit for bit.
 *
 * A file this pass cannot walk is refused, not returned. Handing back the input
 * on a parse failure would re-admit exactly the bytes the caller asked to have
 * removed, and would report a sanitizer that did nothing as a success. Turning a
 * failure into an apparently clean attachment is worse than a rejection the user
 * can see, so malformed input is `{ ok: false }` with a sentence saying what did
 * not add up. The one thing this pass may not do is promise a file is clean
 * because it gave up reading it.
 *
 * Rules are shaped like `SVG_SANITIZER_RULES` next door, and for the same
 * reason: the label is what the user reads, the reason is what the next
 * maintainer reads, and both live in one entry so they cannot drift apart.
 */

export interface RasterMetadataRuleDefinition {
  /** What was taken out, as a short noun phrase a person can read. */
  label: string;
  /** Why the rule exists. Long is fine here, and only here. */
  reason: string;
}

export const RASTER_METADATA_RULES = {
  exif: {
    label: "the camera, device and GPS location data",
    reason:
      "EXIF is where a camera writes the GPS coordinates of the shot, the device model, its serial number and the time it was taken. The JPEG application segments are decided by a whitelist, not by a list of known-bad marker numbers: of the sixteen `APPn` segments only three are named to survive — an APP0 that carries a JFIF header, an APP14 (Adobe's colour transform) and an APP2 that carries an ICC colour profile — because those say how to decode the pixels rather than who produced them. Each survivor is named by its payload, not by its marker number: an APP0 whose payload is `JFXX\0` instead of `JFIF\0` holds an embedded thumbnail, which is a second image inside the file and the classic way a cropped photograph still shows what was cropped out, and an APP2 that is not an ICC profile is a vendor block by definition. Everything else leaves because it was not named, so a vendor block this code has never seen goes with it, and no rule here has to recognise it first. A file with no recognisable signature still leaves under this rule: the segment's whole purpose is metadata, so keeping it on the chance it is benign is the wrong side of the bet.",
  },
  xmp: {
    label: "the XMP metadata packet",
    reason:
      "XMP repeats the identity data above as XML, in a JPEG APP1 segment. Removing EXIF alone would leave a second, complete copy of the same coordinates behind, so a strip that stopped at EXIF would look thorough and not be.",
  },
  iptc: {
    label: "the IPTC and Photoshop block",
    reason:
      "an APP13 block carries captions, credit lines and the author's contact details. It is descriptive data about who made the file, not pixels.",
  },
  comment: {
    label: "a comment",
    reason:
      "a JPEG COM segment is free text an editor or a camera left behind. It has no defined role in rendering and can name a person or a machine.",
  },
  textChunk: {
    label: "a text chunk",
    reason:
      "PNG tEXt, zTXt and iTXt chunks are free text, and iTXt is where an XMP packet lives — the coordinates EXIF would carry can be repeated there in full.",
  },
  timestamp: {
    label: "the modification time",
    reason:
      "a PNG tIME chunk records when the file was last saved. A timestamp on its own is not an identity, but beside the other blocks it fixes when a photograph was taken, and it is not used to draw the image.",
  },
  signature: {
    label: "a digital signature",
    reason:
      "a PNG dSIG chunk is a signature over the image and names the signer. It is not pixel data; it stops verifying the moment any chunk is removed, so carrying it after this pass would carry a proof of a file that no longer exists.",
  },
  trailingData: {
    label: "data appended after the image",
    reason:
      "bytes after the JPEG EOI marker or the PNG IEND chunk are not part of the picture: no decoder reads them, and they are a known place to hide a payload behind a file that still draws. Dropping them keeps an image that renders identically and closes the hiding place; refusing the whole file over them would punish the user for bytes they never needed.",
  },
  unknownChunk: {
    label: "an unrecognised PNG chunk",
    reason:
      "a PNG chunk that is neither one this pass keeps nor one it knows by name is dropped when the PNG specification marks it ancillary — the fifth bit of the first byte of the chunk type: a lowercase first letter means an ancillary chunk, and the specification requires a decoder to be able to ignore those and still draw the image. That is where the safety comes from, and it is worth stating precisely: not from having inspected what the chunk contained, but from the specification declaring that this class of chunk may be skipped. It is what closes the door on private chunks — Adobe's `prVW` embedded preview, Apple's `mkBF`/`mkTS`/`mkBT` blocks, and any type a vendor registers later — none of which this code has to know. A *critical* chunk (uppercase first letter) that is not recognised is a different case and is refused outright, because dropping it would break decoding and keeping it would carry bytes this pass never examined.",
  },
} as const satisfies Record<string, RasterMetadataRuleDefinition>;

export type RasterMetadataRule = keyof typeof RASTER_METADATA_RULES;

export type RasterStripResult =
  | { ok: true; bytes: Uint8Array; removed: readonly RasterMetadataRule[] }
  | { ok: false; reason: string };

/** `a`, `a and b`, `a, b and c`. Mirrors `listWithAnd` in the SVG module. */
function listWithAnd(items: readonly string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

/**
 * One sentence about a file this pass edited, naming what left it.
 *
 * Same contract as `svgSanitizerNotice`: rules are named in declaration order so
 * the same input always reads the same, each is named once however many times it
 * fired, and no rule means no message rather than a sentence about nothing.
 */
export function rasterMetadataNotice(name: string, removed: readonly RasterMetadataRule[]): string {
  const fired = new Set(removed);
  const labels = (Object.keys(RASTER_METADATA_RULES) as RasterMetadataRule[])
    .filter((rule) => fired.has(rule))
    .map((rule) => RASTER_METADATA_RULES[rule].label);
  if (labels.length === 0) return "";
  const verb = labels.length === 1 ? "was removed" : "were removed";
  return `${name} was stripped of metadata before attaching: ${listWithAnd(labels)} ${verb}.`;
}

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] as const;

/**
 * The chunks that survive, named one by one. Everything absent from this set is
 * decided by the PNG specification rather than by a guess: the fifth bit of the
 * first byte of the chunk type separates critical chunks from ancillary ones,
 * and the specification requires a decoder to draw an image while ignoring
 * every ancillary chunk it does not understand.
 *
 * That guarantee is about *decoding*, though, not about *rendering*, and the
 * difference is the whole reason this list is not simply "the four critical
 * chunks". An image still decodes without its colour chunks; it comes out the
 * wrong colour. So every chunk that changes what the eye sees has to be named
 * here, one by one, or the strip quietly becomes an edit.
 *
 * So the list is the whole whitelist, and it is short on purpose. `IHDR`, `PLTE`,
 * `IDAT` and `IEND` are the image. `tRNS`, `gAMA`, `cHRM`, `sRGB`, `iCCP`,
 * `sBIT`, `bKGD`, `hIST`, `pHYs` and `sPLT` all change how the pixels are
 * rendered — transparency, colour space, physical size, suggested palettes — and
 * dropping any of them would alter how the image looks. `cICP`, `mDCv` and
 * `cLLi` are the third-edition HDR colour chunks: on a wide-gamut display an
 * image without them is rendered with different colours than the one that was
 * attached. `acTL`, `fcTL` and
 * `fdAT` are an APNG's animation control and frame data: they are ancillary, so
 * a whitelist that forgot them would leave a valid, still PNG that no longer
 * moves, which is an edit nobody asked for.
 *
 * The six metadata chunks this pass removes are then named to the rule they fire,
 * so the user reads which kind of thing left the file instead of one generic
 * sentence. Anything else is decided by the critical bit.
 */
const PNG_KEPT_CHUNKS: ReadonlySet<string> = new Set([
  "IHDR",
  "PLTE",
  "IDAT",
  "IEND",
  "tRNS",
  "gAMA",
  "cHRM",
  "sRGB",
  "iCCP",
  "sBIT",
  "bKGD",
  "hIST",
  "pHYs",
  "sPLT",
  "cICP",
  "mDCv",
  "cLLi",
  "acTL",
  "fcTL",
  "fdAT",
]);

const PNG_METADATA_CHUNKS: Readonly<Record<string, RasterMetadataRule | undefined>> = {
  eXIf: "exif",
  tEXt: "textChunk",
  zTXt: "textChunk",
  iTXt: "textChunk",
  tIME: "timestamp",
  dSIG: "signature",
};

function asciiPrefix(text: string): readonly number[] {
  return [...text].map((character) => character.charCodeAt(0));
}

const XMP_PREFIX = asciiPrefix("http://ns.adobe.com/xap/1.0/\u0000");
const ICC_PROFILE_PREFIX = asciiPrefix("ICC_PROFILE\u0000");
const JFIF_PREFIX = asciiPrefix("JFIF\u0000");

function matchesAt(bytes: Uint8Array, at: number, prefix: readonly number[]): boolean {
  if (at + prefix.length > bytes.length) return false;
  for (let index = 0; index < prefix.length; index += 1) {
    if (bytes[at + index] !== prefix[index]) return false;
  }
  return true;
}

/**
 * JPEG: from SOI, marker by marker. Every marker is `FF xx`; the markers that
 * carry a payload have a two-byte big-endian length that counts itself.
 *
 * Application segments are decided by a whitelist, not by a list of known-bad
 * marker numbers. Of the sixteen `APPn` markers only three survive, each because
 * it says how to decode the pixels rather than who produced them, and each named
 * by its payload rather than by its marker number: an APP0 that carries a JFIF
 * header (the density and aspect ratio), APP14 (Adobe, whose colour transform a
 * CMYK or YCCK image needs to look like anything at all), and APP2 only when its
 * payload opens with `ICC_PROFILE\0` — an ICC profile is colour. The payload
 * check on APP0 is not decoration: an APP0 whose payload is `JFXX\0` instead
 * carries an embedded thumbnail, a second image inside the file, which is how a
 * cropped photograph goes on showing what was cropped away. Every other `APPn`
 * leaves, under a named rule when the payload identifies itself (`iptc` for
 * APP13, `xmp` for Adobe's XMP signature) and under `exif` otherwise. Naming the
 * survivors is the point: everything else goes because nobody named it, so a
 * vendor block this code has never heard of is removed without having to be
 * recognised first.
 *
 * The walk does read the entropy-coded data, and reading it is not the same as
 * rewriting it. Byte stuffing is a defined structure: inside a scan `FF 00` is a
 * literal `FF`, `FF D0`-`FF D7` are restart markers and `FF FF` is padding, so
 * an `FF` that announces a real marker is unambiguous. Walking it is how the end
 * of a scan is found — and it has to be found, because a progressive JPEG
 * interleaves several scans with segment markers between them, and a segment
 * there would otherwise never be examined at all. What the walk produces is
 * only a length: the bytes of the scan, from the SOS header to the marker that
 * closes it, are copied verbatim, stuffing and restart markers included.
 *
 * The first real marker after a scan is either EOI, where the image ends, or the
 * next segment, where the marker loop resumes. Bytes after EOI are not part of
 * the image and are dropped, because a strip that reads that far and then ships
 * them anyway is the same hiding place it exists to close.
 */
function stripJpegSegments(
  bytes: Uint8Array,
  removed: Set<RasterMetadataRule>,
): { ok: true; bytes: Uint8Array } | { ok: false; reason: string } {
  if (bytes.length < 2 || bytes[0] !== 0xff || bytes[1] !== 0xd8) {
    return { ok: false, reason: "it does not begin with the JPEG start-of-image marker FF D8" };
  }
  const out = new Uint8Array(bytes.length);
  out[0] = 0xff;
  out[1] = 0xd8;
  let written = 2;
  let index = 2;
  let sawScan = false;

  while (index < bytes.length) {
    if (index + 1 >= bytes.length) {
      return {
        ok: false,
        reason: `the marker at byte ${index} is truncated: the file ends after FF`,
      };
    }
    if (bytes[index] !== 0xff) {
      return {
        ok: false,
        reason: `byte ${index} is 0x${bytes[index].toString(16)}, not the FF that begins a marker`,
      };
    }
    const code = bytes[index + 1];
    // A run of FF is padding before the real marker; carry it through one byte at
    // a time so the next iteration can read the marker that follows.
    if (code === 0xff) {
      out[written] = 0xff;
      written += 1;
      index += 1;
      continue;
    }
    if (code === 0x00) {
      return {
        ok: false,
        reason: `byte ${index} begins FF 00 outside the entropy-coded data, where it is not a marker`,
      };
    }
    if (code === 0xd8) {
      return { ok: false, reason: "it contains a second start-of-image marker" };
    }
    if (code === 0xd9) {
      if (!sawScan) {
        return { ok: false, reason: "it ends before its first scan (no SOS marker)" };
      }
      out[written] = 0xff;
      out[written + 1] = 0xd9;
      written += 2;
      index += 2;
      // Anything past EOI is not part of the image: no decoder reads it, and it
      // is exactly the hiding place this walk already caught on the PNG side.
      if (index < bytes.length) {
        removed.add("trailingData");
        index = bytes.length;
      }
      break;
    }
    // RST0-RST7 and TEM have no length field; reading one would consume the two
    // bytes after them as a length and walk off into the next marker.
    if (code === 0x01 || (code >= 0xd0 && code <= 0xd7)) {
      out[written] = 0xff;
      out[written + 1] = code;
      written += 2;
      index += 2;
      continue;
    }
    if (index + 3 >= bytes.length) {
      return {
        ok: false,
        reason: `the segment at byte ${index} is truncated before its length field`,
      };
    }
    const length = (bytes[index + 2] << 8) | bytes[index + 3];
    if (length < 2) {
      return {
        ok: false,
        reason: `the segment at byte ${index} declares length ${length}, which cannot be less than 2`,
      };
    }
    const segmentEnd = index + 2 + length;
    if (segmentEnd > bytes.length) {
      return {
        ok: false,
        reason: `the segment at byte ${index} declares ${length} bytes but only ${bytes.length - index - 2} are left in the file`,
      };
    }
    if (code === 0xda) {
      // Start of scan. The header is copied now; the entropy-coded data is copied
      // once the walk below has found where it ends.
      out.set(bytes.subarray(index, segmentEnd), written);
      written += segmentEnd - index;
      let scan = segmentEnd;
      let closedByMarker = false;
      while (scan < bytes.length) {
        if (bytes[scan] !== 0xff) {
          scan += 1;
          continue;
        }
        if (scan + 1 >= bytes.length) {
          // A lone FF as the last byte starts no marker, so reading past it would
          // leave the buffer. It is a truncation, not a marker.
          return {
            ok: false,
            reason: `it ends on a lone FF byte inside the entropy-coded data, which begins no marker`,
          };
        }
        const next = bytes[scan + 1];
        if (next === 0x00) {
          // FF 00 is a literal FF in the scan, not a marker.
          scan += 2;
          continue;
        }
        if (next === 0xff) {
          // Fill byte before the real marker; step over it one at a time.
          scan += 1;
          continue;
        }
        if (next >= 0xd0 && next <= 0xd7) {
          // RST0-RST7 restart the entropy coder inside the scan.
          scan += 2;
          continue;
        }
        closedByMarker = true;
        break;
      }
      if (!closedByMarker) {
        return {
          ok: false,
          reason: "it finishes inside the entropy-coded data of a scan without an EOI marker",
        };
      }
      // The scan's bytes are copied verbatim; the walk only measured them.
      out.set(bytes.subarray(segmentEnd, scan), written);
      written += scan - segmentEnd;
      index = scan;
      sawScan = true;
      // A progressive JPEG resumes the marker loop here, which is also what lets
      // an APPn placed between two scans be examined instead of skipped.
      continue;
    }
    const payload = index + 4;
    if (code >= 0xe0 && code <= 0xef) {
      // APPn: keep the three named survivors, drop everything else.
      const survives =
        (code === 0xe0 && matchesAt(bytes, payload, JFIF_PREFIX)) ||
        code === 0xee ||
        (code === 0xe2 && matchesAt(bytes, payload, ICC_PROFILE_PREFIX));
      if (!survives) {
        if (code === 0xed) removed.add("iptc");
        else if (matchesAt(bytes, payload, XMP_PREFIX)) removed.add("xmp");
        else removed.add("exif");
        index = segmentEnd;
        continue;
      }
    }
    if (code === 0xfe) {
      removed.add("comment");
      index = segmentEnd;
      continue;
    }
    // Everything else — APP0, APP14, an ICC APP2, DQT, DHT, SOF*, DRI — is
    // copied byte for byte.
    out.set(bytes.subarray(index, segmentEnd), written);
    written += segmentEnd - index;
    index = segmentEnd;
  }

  if (!sawScan) {
    return { ok: false, reason: "it ends without a start-of-scan marker" };
  }
  if (removed.size === 0) return { ok: true, bytes };
  return { ok: true, bytes: out.subarray(0, written) };
}

/**
 * PNG: the eight-byte signature, then chunks of `length | type | data | CRC`.
 * The chunks that survive are named in `PNG_KEPT_CHUNKS` and copied with their
 * own CRC still attached; a chunk the pass knows is identity leaves under its
 * own rule; and any other chunk is decided by the critical bit — ancillary ones
 * go, critical ones make the whole file unreadable. The whitelist is what makes
 * a private chunk (`prVW`, `mkBF`, anything registered later) leave without this
 * code having to know the type, exactly as the JPEG side now does for `APPn`.
 *
 * The CRC is never recomputed, and that is deliberate: a recomputed CRC would
 * ratify an accidental edit to the bytes it covers, and the corruption would
 * come out looking valid.
 */
function stripPngChunks(
  bytes: Uint8Array,
  removed: Set<RasterMetadataRule>,
): { ok: true; bytes: Uint8Array } | { ok: false; reason: string } {
  if (bytes.length < 8 || !PNG_SIGNATURE.every((byte, index) => bytes[index] === byte)) {
    return { ok: false, reason: "it does not begin with the eight-byte PNG signature" };
  }
  const out = new Uint8Array(bytes.length);
  out.set(PNG_SIGNATURE, 0);
  let written = 8;
  let index = 8;
  let sawEnd = false;

  while (index < bytes.length) {
    if (index + 8 > bytes.length) {
      return { ok: false, reason: `the chunk header at byte ${index} is truncated` };
    }
    const length =
      bytes[index] * 0x1000000 +
      bytes[index + 1] * 0x10000 +
      bytes[index + 2] * 0x100 +
      bytes[index + 3];
    const type = String.fromCharCode(
      bytes[index + 4],
      bytes[index + 5],
      bytes[index + 6],
      bytes[index + 7],
    );
    const chunkEnd = index + 8 + length + 4;
    if (chunkEnd > bytes.length) {
      return {
        ok: false,
        reason: `the ${type} chunk at byte ${index} declares ${length} data bytes and runs past the end of the file`,
      };
    }
    const rule = PNG_METADATA_CHUNKS[type];
    if (PNG_KEPT_CHUNKS.has(type)) {
      out.set(bytes.subarray(index, chunkEnd), written);
      written += chunkEnd - index;
    } else if (rule !== undefined) {
      removed.add(rule);
    } else {
      // The critical bit: the fifth bit of the first byte of the type. Lowercase
      // first letter means ancillary, and the specification requires a decoder to
      // be able to ignore those, so dropping one cannot change the picture. That
      // is a property of the format, not a judgement about the contents.
      const ancillary = (bytes[index + 4] & 0x20) !== 0;
      if (!ancillary) {
        return {
          ok: false,
          reason: `it carries a critical ${type} chunk, which this pass does not know how to keep or remove`,
        };
      }
      removed.add("unknownChunk");
    }
    index = chunkEnd;
    if (type === "IEND") {
      sawEnd = true;
      break;
    }
  }

  if (!sawEnd) return { ok: false, reason: "it ends without an IEND chunk" };
  if (index !== bytes.length) {
    // Bytes after IEND are not a chunk and cannot be read as one: a metadata blob
    // appended after the image is a known hiding place. They are dropped rather
    // than refused — the picture up to IEND is whole and draws identically, and
    // rejecting the file would cost the user an image over bytes nobody needed.
    removed.add("trailingData");
  }
  if (removed.size === 0) return { ok: true, bytes };
  return { ok: true, bytes: out.subarray(0, written) };
}

/**
 * Returns the same image with its identity metadata removed, or a sentence
 * saying why the bytes could not be walked. The measured mime decides which
 * container is parsed; a caller that has not sniffed the bytes should not be
 * calling this at all.
 *
 * When no rule fires, the original array is returned unchanged rather than a
 * rebuilt copy, so an image that carried nothing is not rewritten for no reason.
 */
export function stripRasterMetadata(bytes: Uint8Array, mime: RasterMimeType): RasterStripResult {
  const removed = new Set<RasterMetadataRule>();
  const stripped =
    mime === "image/png" ? stripPngChunks(bytes, removed) : stripJpegSegments(bytes, removed);
  if (!stripped.ok) return stripped;
  return { ok: true, bytes: stripped.bytes, removed: [...removed] };
}
