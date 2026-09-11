// @vitest-environment node

import { describe, expect, it } from "vitest";
import { RASTER_METADATA_RULES, rasterMetadataNotice, stripRasterMetadata } from "./rasterMetadata";

/** Concatenates the byte runs of a hand-built container. */
function bytes(...parts: readonly (readonly number[])[]): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(parts.flat());
}

function ascii(text: string): number[] {
  return [...text].map((character) => character.charCodeAt(0));
}

/** One JPEG marker with a payload; the length field counts itself. */
function jpegSegment(marker: number, payload: readonly number[]): number[] {
  const length = payload.length + 2;
  return [0xff, marker, (length >> 8) & 0xff, length & 0xff, ...payload];
}

/**
 * One PNG chunk. The CRC is not a real checksum — it is four distinct bytes so a
 * surviving chunk can be checked for identity, which is exactly the property the
 * stripper promises: it never recomputes one.
 */
function pngChunk(type: string, data: readonly number[], crc = [0xde, 0xad, 0xbe, 0xef]): number[] {
  const length = data.length;
  return [
    (length >>> 24) & 0xff,
    (length >>> 16) & 0xff,
    (length >>> 8) & 0xff,
    length & 0xff,
    ...ascii(type),
    ...data,
    ...crc,
  ];
}

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

const IHDR_DATA = [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0];
const IDAT_DATA = [0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01];
const IEND_DATA: number[] = [];

const EXIF_PAYLOAD = [
  ...ascii("Exif\u0000\u0000"),
  ...ascii("GPSLatitude"),
  0x01,
  0x02,
  0x03,
  0x04,
];
const XMP_PAYLOAD = [
  ...ascii("http://ns.adobe.com/xap/1.0/\u0000"),
  ...ascii("<x:xmpmeta>51.5,-0.12</x:xmpmeta>"),
];
const ICC_PAYLOAD = [...ascii("ICC_PROFILE\u0000"), 0x01, 0x01, ...ascii("fake icc bytes")];
const JFIF_PAYLOAD = [...ascii("JFIF\u0000"), 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00];
const ADOBE_PAYLOAD = [...ascii("Adobe"), 0x00, 0x64, 0x00, 0x00, 0x00, 0x00];
const DQT_PAYLOAD = [0x00, ...new Array<number>(64).fill(0x08)];
const DHT_PAYLOAD = [0x00, 0x01, 0x02, 0x03];
const SOS_HEADER = [0x01, 0x01, 0x00, 0x00, 0x3f, 0x00];
/** Entropy-coded data: `FF 00` stuffing and a bare RST0 marker inside it. */
const ENTROPY = [0x12, 0x34, 0xff, 0x00, 0x56, 0xff, 0xd0, 0x78];
const EOI = [0xff, 0xd9];

const SOS_SEGMENT = jpegSegment(0xda, SOS_HEADER);

function indexOfSequence(haystack: Uint8Array, needle: readonly number[]): number {
  if (needle.length === 0) return 0;
  outer: for (let start = 0; start + needle.length <= haystack.length; start += 1) {
    for (let offset = 0; offset < needle.length; offset += 1) {
      if (haystack[start + offset] !== needle[offset]) continue outer;
    }
    return start;
  }
  return -1;
}

describe("JPEG: the identity segments leave, the picture stays", () => {
  const WITH_EXIF = bytes(
    [0xff, 0xd8],
    jpegSegment(0xe0, JFIF_PAYLOAD),
    jpegSegment(0xe1, EXIF_PAYLOAD),
    jpegSegment(0xe2, ICC_PAYLOAD),
    jpegSegment(0xdb, DQT_PAYLOAD),
    SOS_SEGMENT,
    ENTROPY,
    EOI,
  );

  it("removes the EXIF APP1 and reports it", () => {
    const result = stripRasterMetadata(WITH_EXIF, "image/jpeg");
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    expect(result.removed).toEqual(["exif"]);
    expect(indexOfSequence(result.bytes, ascii("GPSLatitude"))).toBe(-1);
    expect(indexOfSequence(result.bytes, EXIF_PAYLOAD)).toBe(-1);
  });

  it("keeps APP0, the ICC APP2 and DQT byte-identical, in order", () => {
    const result = stripRasterMetadata(WITH_EXIF, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    const expected = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xe2, ICC_PAYLOAD),
      jpegSegment(0xdb, DQT_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    expect([...result.bytes]).toEqual([...expected]);
  });

  it("copies the whole scan verbatim, FF 00 stuffing, RST marker and EOI included", () => {
    const result = stripRasterMetadata(WITH_EXIF, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    const sos = indexOfSequence(result.bytes, [0xff, 0xda]);
    expect(sos).toBeGreaterThanOrEqual(0);
    expect([...result.bytes.subarray(sos)]).toEqual([...SOS_SEGMENT, ...ENTROPY, ...EOI]);
  });

  it("removes an APP13 and a COM, and keeps APP14", () => {
    const input = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xed, [...ascii("Photoshop 3.0\u0000"), ...ascii("Credit: someone")]),
      jpegSegment(0xfe, ascii("shot on a phone")),
      jpegSegment(0xee, ADOBE_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    const result = stripRasterMetadata(input, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    expect([...result.removed].sort()).toEqual(["comment", "iptc"]);
    expect(indexOfSequence(result.bytes, ascii("Credit: someone"))).toBe(-1);
    expect(indexOfSequence(result.bytes, ascii("shot on a phone"))).toBe(-1);
    expect(indexOfSequence(result.bytes, ADOBE_PAYLOAD)).toBeGreaterThanOrEqual(0);
  });

  it("removes a non-ICC APP2 as identity but keeps the ICC one", () => {
    const result = stripRasterMetadata(
      bytes(
        [0xff, 0xd8],
        jpegSegment(0xe2, [...ascii("SomeVendorIdentity"), 0x01]),
        SOS_SEGMENT,
        ENTROPY,
        EOI,
      ),
      "image/jpeg",
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["exif"]);
    expect(indexOfSequence(result.bytes, ascii("SomeVendorIdentity"))).toBe(-1);
  });

  it("names XMP as XMP when the APP1 signature is Adobe's", () => {
    const result = stripRasterMetadata(
      bytes([0xff, 0xd8], jpegSegment(0xe1, XMP_PAYLOAD), SOS_SEGMENT, ENTROPY, EOI),
      "image/jpeg",
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["xmp"]);
    expect(indexOfSequence(result.bytes, ascii("xmpmeta"))).toBe(-1);
  });

  it("drops every APPn that was not named, and keeps the three that were", () => {
    // APP3 (Meta/Kodak), APP12 (Ducky/PictureInfo) and APP15 are vendor blocks
    // carrying the camera's settings. A blacklist walks straight past them; the
    // whitelist removes them without having to know what they are.
    const input = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xe2, ICC_PAYLOAD),
      jpegSegment(0xe3, [...ascii("Meta"), 0x00, ...ascii("shutter 1/250")]),
      jpegSegment(0xec, [...ascii("Ducky"), ...ascii("iso 800")]),
      jpegSegment(0xee, ADOBE_PAYLOAD),
      jpegSegment(0xef, [...ascii("Vendor XV"), 0x01, 0x02]),
      jpegSegment(0xdb, DQT_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    const result = stripRasterMetadata(input, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["exif"]);
    // APP0, the ICC APP2, APP14 and DQT also survive — but the three survivors
    // that the whitelist names are the ones asserted byte for byte.
    const expected = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xe2, ICC_PAYLOAD),
      jpegSegment(0xee, ADOBE_PAYLOAD),
      jpegSegment(0xdb, DQT_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    expect([...result.bytes]).toEqual([...expected]);
    expect(indexOfSequence(result.bytes, ascii("shutter 1/250"))).toBe(-1);
    expect(indexOfSequence(result.bytes, ascii("Ducky"))).toBe(-1);
    expect(indexOfSequence(result.bytes, ascii("Vendor XV"))).toBe(-1);
  });

  it("walks a progressive JPEG's second scan, and removes an APP1 between the scans", () => {
    // The segment between the two scans is invisible to a walker that stops at
    // the first SOS, which is what this used to do.
    const input = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xdb, DQT_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      jpegSegment(0xc4, DHT_PAYLOAD),
      jpegSegment(0xe1, EXIF_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    const result = stripRasterMetadata(input, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["exif"]);
    const expected = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xdb, DQT_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      jpegSegment(0xc4, DHT_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    // Both scans' bytes survive identically: the walk measured them, it did not
    // rewrite them.
    expect([...result.bytes]).toEqual([...expected]);
    expect(indexOfSequence(result.bytes, ascii("GPSLatitude"))).toBe(-1);
  });

  it("walks stuffing, restart markers and FF fill without stopping, and copies the scan intact", () => {
    // FF 00 is a literal FF, FF D0-FF D7 restart the entropy coder, and FF FF is
    // padding before the real marker. The walk steps over all three; a reader
    // that stopped at the first FF would end the scan in the wrong place.
    const trickyScan = [
      0x01, 0xff, 0x00, 0x02, 0xff, 0xd0, 0x03, 0xff, 0xd1, 0x04, 0xff, 0xd2, 0x05, 0xff, 0xd3,
      0x06, 0xff, 0xd4, 0x07, 0xff, 0xd5, 0x08, 0xff, 0xd6, 0x09, 0xff, 0xd7, 0x0a, 0xff, 0xff,
    ];
    const input = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xe1, EXIF_PAYLOAD),
      SOS_SEGMENT,
      trickyScan,
      EOI,
    );
    const result = stripRasterMetadata(input, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["exif"]);
    const sos = indexOfSequence(result.bytes, [0xff, 0xda]);
    expect(sos).toBeGreaterThanOrEqual(0);
    expect([...result.bytes.subarray(sos)]).toEqual([...SOS_SEGMENT, ...trickyScan, ...EOI]);
  });

  it("drops bytes appended after EOI instead of shipping them", () => {
    const blob = ascii("appended EXIF blob with a GPS fix");
    const input = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
      blob,
    );
    const result = stripRasterMetadata(input, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["trailingData"]);
    expect(indexOfSequence(result.bytes, blob)).toBe(-1);
    expect([...result.bytes]).toEqual([
      ...[0xff, 0xd8],
      ...jpegSegment(0xe0, JFIF_PAYLOAD),
      ...SOS_SEGMENT,
      ...ENTROPY,
      ...EOI,
    ]);
    expect(result.bytes.length).toBe(input.length - blob.length);
  });

  it("keeps an APP0 only when its payload is a JFIF header", () => {
    // An APP0 whose payload is JFXX\0 carries an embedded thumbnail — a second
    // image inside the file, and the classic way a cropped photograph goes on
    // showing what was cropped away. The survivor is named by content, not by
    // marker number, so both APP0s are decided by what they say they are.
    const jfxxPayload = [...ascii("JFXX\u0000"), 0x10, ...ascii("tiny jpeg here")];
    const input = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xe0, jfxxPayload),
      jpegSegment(0xee, ADOBE_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    const result = stripRasterMetadata(input, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["exif"]);
    expect(indexOfSequence(result.bytes, ascii("JFXX"))).toBe(-1);
    expect(indexOfSequence(result.bytes, ascii("tiny jpeg here"))).toBe(-1);
    const expected = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xee, ADOBE_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    expect([...result.bytes]).toEqual([...expected]);
  });

  it("returns a clean JPEG unchanged, by reference", () => {
    const clean = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      jpegSegment(0xdb, DQT_PAYLOAD),
      SOS_SEGMENT,
      ENTROPY,
      EOI,
    );
    const result = stripRasterMetadata(clean, "image/jpeg");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual([]);
    // Identity, not equality: a file with nothing to remove is not rewritten.
    expect(result.bytes).toBe(clean);
  });
});

describe("PNG: only the identity chunks leave, CRCs stay attached", () => {
  const WITH_METADATA = bytes(
    PNG_SIGNATURE,
    pngChunk("IHDR", IHDR_DATA),
    pngChunk("eXIf", EXIF_PAYLOAD),
    pngChunk("iTXt", [...ascii("XML:com.adobe.xmp\u0000"), ...ascii("<x:xmpmeta>51.5,-0.12")]),
    pngChunk("iCCP", [...ascii("ICC Profile\u0000"), 0x00, ...ascii("fake")]),
    pngChunk("IDAT", IDAT_DATA),
    pngChunk("IEND", IEND_DATA),
  );

  it("drops eXIf and iTXt and preserves every surviving chunk byte for byte", () => {
    const result = stripRasterMetadata(WITH_METADATA, "image/png");
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    expect([...result.removed].sort()).toEqual(["exif", "textChunk"]);
    const expected = bytes(
      PNG_SIGNATURE,
      pngChunk("IHDR", IHDR_DATA),
      pngChunk("iCCP", [...ascii("ICC Profile\u0000"), 0x00, ...ascii("fake")]),
      pngChunk("IDAT", IDAT_DATA),
      pngChunk("IEND", IEND_DATA),
    );
    // Full equality is the assertion: IHDR, iCCP, IDAT and IEND survive in order,
    // with the CRCs this module never recomputed.
    expect([...result.bytes]).toEqual([...expected]);
  });

  it("keeps an APNG's animation chunks so the image still moves", () => {
    const apng = bytes(
      PNG_SIGNATURE,
      pngChunk("IHDR", IHDR_DATA),
      pngChunk("acTL", [0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00]),
      pngChunk("fcTL", [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]),
      pngChunk("IDAT", IDAT_DATA),
      pngChunk("fdAT", [0x00, 0x00, 0x00, 0x01, 0x99, 0x88]),
      pngChunk("IEND", IEND_DATA),
    );
    const result = stripRasterMetadata(apng, "image/png");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual([]);
    expect(result.bytes).toBe(apng);
  });

  it("drops tIME as a timestamp and dSIG as a signature", () => {
    const result = stripRasterMetadata(
      bytes(
        PNG_SIGNATURE,
        pngChunk("IHDR", IHDR_DATA),
        pngChunk("tIME", [0x07, 0xe8, 0x01, 0x02, 0x0c, 0x1e, 0x2d]),
        pngChunk("dSIG", [...ascii("signer: alice")]),
        pngChunk("IDAT", IDAT_DATA),
        pngChunk("IEND", IEND_DATA),
      ),
      "image/png",
    );
    if (!result.ok) throw new Error(result.reason);

    expect([...result.removed].sort()).toEqual(["signature", "timestamp"]);
    expect(indexOfSequence(result.bytes, ascii("signer: alice"))).toBe(-1);
  });

  it("keeps the rendering chunks that are easy to forget", () => {
    // sBIT (significant bits), bKGD (background), pHYs (physical size) and sPLT
    // (suggested palette) all change how the pixels are presented. They are on
    // the whitelist deliberately: a later "simplification" that dropped the less
    // famous ones would alter the picture.
    const input = bytes(
      PNG_SIGNATURE,
      pngChunk("IHDR", IHDR_DATA),
      pngChunk("sBIT", [0x08, 0x08, 0x08]),
      pngChunk("bKGD", [0x00, 0x01, 0x00, 0x02, 0x00, 0x03]),
      pngChunk("pHYs", [0x00, 0x00, 0x0b, 0x13, 0x00, 0x00, 0x0b, 0x13, 0x01]),
      // cICP, mDCv and cLLi are the third edition's HDR colour chunks. An image
      // decodes perfectly well without them and comes out a different colour on
      // a wide-gamut display: the case the "an ancillary chunk may be ignored"
      // rule does not cover, because the specification promises the image still
      // decodes, not that it still looks the same.
      pngChunk("cICP", [0x09, 0x10, 0x00, 0x01]),
      pngChunk("mDCv", [0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x04]),
      pngChunk("cLLi", [0x00, 0x00, 0x27, 0x10, 0x00, 0x00, 0x03, 0xe8]),
      pngChunk("sPLT", [...ascii("palette\u0000"), 0x08, 0x01, 0x11]),
      pngChunk("IDAT", IDAT_DATA),
      pngChunk("IEND", IEND_DATA),
    );
    const result = stripRasterMetadata(input, "image/png");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual([]);
    expect([...result.bytes]).toEqual([...input]);
  });

  it("drops a private ancillary chunk it has never heard of", () => {
    // prVW is Adobe's embedded preview, and a vendor-registered type is the same
    // case. Neither is on the whitelist, and the specification says an ancillary
    // chunk may be ignored, so dropping it is safe without knowing its contents.
    const prvw = pngChunk("prVW", ascii("tiny preview jpeg"));
    const withPrivate = bytes(
      PNG_SIGNATURE,
      pngChunk("IHDR", IHDR_DATA),
      prvw,
      pngChunk("IDAT", IDAT_DATA),
      pngChunk("IEND", IEND_DATA),
    );
    const result = stripRasterMetadata(withPrivate, "image/png");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["unknownChunk"]);
    expect(indexOfSequence(result.bytes, ascii("tiny preview jpeg"))).toBe(-1);
    expect([...result.bytes]).toEqual([
      ...PNG_SIGNATURE,
      ...pngChunk("IHDR", IHDR_DATA),
      ...pngChunk("IDAT", IDAT_DATA),
      ...pngChunk("IEND", IEND_DATA),
    ]);
  });

  it("refuses a PNG with an unknown critical chunk, naming it", () => {
    // Uppercase first letter: dropping it would break decoding, keeping it would
    // carry bytes this pass never examined. Neither is acceptable.
    const input = bytes(
      PNG_SIGNATURE,
      pngChunk("IHDR", IHDR_DATA),
      pngChunk("ZZZZ", [0x01, 0x02, 0x03]),
      pngChunk("IDAT", IDAT_DATA),
      pngChunk("IEND", IEND_DATA),
    );
    const result = stripRasterMetadata(input, "image/png");

    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.reason).toContain("ZZZZ");
    expect(result.reason).toContain("critical");
  });

  it("keeps a PNG with bytes after IEND, and drops the bytes", () => {
    // The image up to IEND is whole and draws identically; the tail is the same
    // hiding place the JPEG side drops. Refusing the file would cost the user an
    // image over bytes they never needed.
    const blob = ascii("appended EXIF blob");
    const withTail = bytes(
      PNG_SIGNATURE,
      pngChunk("IHDR", IHDR_DATA),
      pngChunk("IDAT", IDAT_DATA),
      pngChunk("IEND", IEND_DATA),
      blob,
    );
    const result = stripRasterMetadata(withTail, "image/png");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual(["trailingData"]);
    expect(indexOfSequence(result.bytes, blob)).toBe(-1);
    // Everything up to and including IEND, CRCs untouched.
    expect([...result.bytes]).toEqual([
      ...PNG_SIGNATURE,
      ...pngChunk("IHDR", IHDR_DATA),
      ...pngChunk("IDAT", IDAT_DATA),
      ...pngChunk("IEND", IEND_DATA),
    ]);
    expect(result.bytes.length).toBe(withTail.length - blob.length);
  });

  it("returns a clean PNG unchanged, by reference", () => {
    const clean = bytes(
      PNG_SIGNATURE,
      pngChunk("IHDR", IHDR_DATA),
      pngChunk("IDAT", IDAT_DATA),
      pngChunk("IEND", IEND_DATA),
    );
    const result = stripRasterMetadata(clean, "image/png");
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual([]);
    expect(result.bytes).toBe(clean);
  });
});

describe("malformed input is refused, never returned whole", () => {
  it("refuses a JPEG truncated in the middle of a segment", () => {
    const full = bytes([0xff, 0xd8], jpegSegment(0xe1, EXIF_PAYLOAD));
    const result = stripRasterMetadata(full.subarray(0, 8), "image/jpeg");

    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.reason).toContain("declares");
  });

  it("refuses a JPEG whose scan is never reached", () => {
    const result = stripRasterMetadata(
      bytes([0xff, 0xd8], jpegSegment(0xe0, JFIF_PAYLOAD)),
      "image/jpeg",
    );
    expect(result).toMatchObject({ ok: false });
  });

  it("refuses a JPEG that does not open with FF D8", () => {
    expect(stripRasterMetadata(bytes([0xff, 0xe1, 0x00, 0x02]), "image/jpeg")).toMatchObject({
      ok: false,
    });
  });

  it("refuses a JPEG whose scan is not closed by a marker", () => {
    const truncated = bytes([0xff, 0xd8], jpegSegment(0xe0, JFIF_PAYLOAD), SOS_SEGMENT, ENTROPY);
    const result = stripRasterMetadata(truncated, "image/jpeg");

    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.reason).toContain("without an EOI marker");
  });

  it("refuses a JPEG whose last byte is a lone FF, without reading past the buffer", () => {
    const truncated = bytes(
      [0xff, 0xd8],
      jpegSegment(0xe0, JFIF_PAYLOAD),
      SOS_SEGMENT,
      [0x12, 0xff],
    );
    const result = stripRasterMetadata(truncated, "image/jpeg");

    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.reason).toContain("lone FF");
  });

  it("refuses a PNG whose chunk length runs past the buffer", () => {
    const result = stripRasterMetadata(
      bytes(PNG_SIGNATURE, [0x7f, 0xff, 0xff, 0xff], ascii("eXIf"), [0x00, 0x00, 0x00, 0x00]),
      "image/png",
    );
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.reason).toContain("runs past the end");
  });

  it("refuses a PNG whose chunk header is cut short", () => {
    expect(
      stripRasterMetadata(bytes(PNG_SIGNATURE, [0x00, 0x00, 0x00]), "image/png"),
    ).toMatchObject({
      ok: false,
    });
  });

  it("refuses a PNG with the wrong signature", () => {
    const result = stripRasterMetadata(
      bytes([0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07], pngChunk("IEND", [])),
      "image/png",
    );
    expect(result.ok).toBe(false);
  });

  it("refuses a PNG that ends without an IEND chunk", () => {
    const result = stripRasterMetadata(
      bytes(PNG_SIGNATURE, pngChunk("IHDR", IHDR_DATA), pngChunk("IDAT", IDAT_DATA)),
      "image/png",
    );
    expect(result).toMatchObject({ ok: false });
    if (result.ok) return;
    expect(result.reason).toContain("IEND");
  });
});

describe("the rule table and the sentence built from it", () => {
  it("gives every rule a label and a reason, from one definition", () => {
    for (const [rule, definition] of Object.entries(RASTER_METADATA_RULES)) {
      expect(definition.label, rule).not.toBe("");
      expect(definition.reason, rule).not.toBe("");
      expect(definition.label, rule).not.toBe(definition.reason);
    }
  });

  it("names the fired rules once each, in declaration order", () => {
    expect(rasterMetadataNotice("shot.jpg", ["comment", "exif"])).toBe(
      "shot.jpg was stripped of metadata before attaching: the camera, device and GPS location data and a comment were removed.",
    );
  });

  it("reads as a list of one rather than a broken list", () => {
    expect(rasterMetadataNotice("shot.jpg", ["exif"])).toBe(
      "shot.jpg was stripped of metadata before attaching: the camera, device and GPS location data was removed.",
    );
  });

  it("has nothing to say when no rule fired", () => {
    expect(rasterMetadataNotice("shot.jpg", [])).toBe("");
  });
});
